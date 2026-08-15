// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Launch-ref classification and materialization before Main handoff.

use crate::arcade_catalog::LaunchTarget;
#[cfg(feature = "bench-tools")]
use crate::library_db;
use flate2::read::DeflateDecoder;
use std::cell::Cell;
use std::fmt;
use std::fs::{self, File};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(feature = "bench-tools")]
use std::time::Instant;

const VIRTUAL_LAUNCH_PREFIX: &str = "magik-plan:";
const AMIGAVISION_GAME_LAUNCH_PREFIX: &str = "magik-amigavision:";
const AMIGAVISION_LAUNCHER_REF: &str = "magik-amigavision-launcher";
const AMIGAVISION_HDF_NAMES: &[&str] = &["AmigaVision.hdf", "MegaAGS.hdf"];
const AMIGAVISION_MGL_NAMES: &[&str] = &["Amiga.mgl", "Amiga 500.mgl", "MegaAGS.mgl"];
const ARCHIVE_STAGE_DIR: &str = "/tmp/mister-magik/launch-payloads";
const MAX_ARCHIVE_MEMBER_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchPreparationFailureKind {
    MissingPayload,
    UnreadablePayload,
    DamagedArchive,
    UnsupportedArchive,
    OversizedArchiveMember,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchPreparationError {
    pub kind: LaunchPreparationFailureKind,
    pub detail: String,
}

impl LaunchPreparationError {
    fn new(kind: LaunchPreparationFailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for LaunchPreparationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.detail.fmt(f)
    }
}

impl std::error::Error for LaunchPreparationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AmigaVisionInstall {
    mgl_path: PathBuf,
    hdf_path: PathBuf,
    shared_dir: PathBuf,
    ags_boot_path: PathBuf,
    games_listing: PathBuf,
    demos_listing: PathBuf,
}

#[cfg(any(feature = "bench-tools", test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LaunchPrepDescriptorStats {
    written: u64,
    skipped: u64,
    bytes: u64,
}

thread_local! {
    static DESCRIPTOR_WRITTEN: Cell<u64> = const { Cell::new(0) };
    static DESCRIPTOR_SKIPPED: Cell<u64> = const { Cell::new(0) };
    static DESCRIPTOR_BYTES: Cell<u64> = const { Cell::new(0) };
}

pub fn prepare_launch_ref(launch_ref: &str) -> Result<String, String> {
    let mut fault_control =
        mister_magik_mister_runtime::direct_reset_fault::process_fault_control();
    prepare_launch_ref_with_fault_control(launch_ref, &mut fault_control)
}

pub fn prepare_launch_ref_with_fault_control(
    launch_ref: &str,
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<String, String> {
    let roots = mister_magik_catalog::catalog_config::library_roots_from_env();
    prepare_launch_ref_with_roots_and_fault_control(launch_ref, &roots, fault_control)
}

pub fn prepare_launch_target(
    launch_target: &LaunchTarget,
) -> Result<LaunchTarget, LaunchPreparationError> {
    let mut fault_control =
        mister_magik_mister_runtime::direct_reset_fault::process_fault_control();
    prepare_launch_target_with_fault_control(launch_target, &mut fault_control)
}

pub fn prepare_launch_target_with_fault_control(
    launch_target: &LaunchTarget,
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<LaunchTarget, LaunchPreparationError> {
    let _lease = mister_magik_catalog::work_coordinator::foreground("launch-preparation");
    let roots = mister_magik_catalog::catalog_config::library_roots_from_env();
    prepare_launch_target_with_roots_and_fault_control(launch_target, &roots, fault_control)
}

fn prepare_launch_target_with_roots(
    launch_target: &LaunchTarget,
    roots: &[String],
) -> Result<LaunchTarget, LaunchPreparationError> {
    let mut fault_control = mister_magik_catalog::fs_fault::NoopDirectResetFaultControl;
    prepare_launch_target_with_roots_and_fault_control(launch_target, roots, &mut fault_control)
}

fn prepare_launch_target_with_roots_and_fault_control(
    launch_target: &LaunchTarget,
    roots: &[String],
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<LaunchTarget, LaunchPreparationError> {
    match launch_target {
        LaunchTarget::Prepared(selection) => Ok(LaunchTarget::Path(
            prepare_launch_ref_with_roots_and_fault_control(
                &selection.launch_ref,
                roots,
                fault_control,
            )
            .map_err(unreadable_payload_error)?
            .into(),
        )),
        LaunchTarget::Path(path) => {
            mister_magik_catalog::prepared_collections::validate_prepared_launch_path(Path::new(
                path.as_ref(),
            ))
            .map_err(unreadable_payload_error)?;
            Ok(launch_target.clone())
        }
        LaunchTarget::Structured(plan) => {
            if let Some(member) = mister_magik_catalog::archive_member::decode_archive_member_ref(
                plan.payload_path.as_ref(),
            )
            .map_err(|detail| {
                LaunchPreparationError::new(LaunchPreparationFailureKind::DamagedArchive, detail)
            })? {
                let payload_path = extract_archive_member(&member, Path::new(ARCHIVE_STAGE_DIR))?;
                let mut prepared = plan.clone();
                prepared.payload_path = payload_path.to_string_lossy().into_owned().into();
                return Ok(LaunchTarget::Structured(prepared));
            }
            mister_magik_catalog::prepared_collections::validate_prepared_launch_path(Path::new(
                plan.payload_path.as_ref(),
            ))
            .map_err(unreadable_payload_error)?;
            Ok(launch_target.clone())
        }
        other => Ok(other.clone()),
    }
}

fn unreadable_payload_error(detail: String) -> LaunchPreparationError {
    let kind = if detail.to_ascii_lowercase().contains("missing")
        || detail.to_ascii_lowercase().contains("not found")
    {
        LaunchPreparationFailureKind::MissingPayload
    } else {
        LaunchPreparationFailureKind::UnreadablePayload
    };
    LaunchPreparationError::new(kind, detail)
}

pub fn cleanup_archive_launch_staging() {
    cleanup_archive_launch_staging_at(Path::new(ARCHIVE_STAGE_DIR));
}

fn cleanup_archive_launch_staging_at(stage_dir: &Path) {
    match fs::remove_dir_all(stage_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => crate::ui_errln!(
            "warning: failed to clean archive launch staging {}: {error}",
            stage_dir.display()
        ),
    }
}

fn extract_archive_member(
    member: &mister_magik_catalog::archive_member::ArchiveMemberRef,
    stage_dir: &Path,
) -> Result<PathBuf, LaunchPreparationError> {
    let _pmu = mister_magik_perf_events::sampled_span("launch.archive-extraction");
    if member.uncompressed_size > MAX_ARCHIVE_MEMBER_BYTES
        || member.compressed_size > MAX_ARCHIVE_MEMBER_BYTES
    {
        return Err(LaunchPreparationError::new(
            LaunchPreparationFailureKind::OversizedArchiveMember,
            format!(
                "archive member exceeds {} bytes: {}::{}",
                MAX_ARCHIVE_MEMBER_BYTES, member.archive_path, member.member_path
            ),
        ));
    }
    if !matches!(member.compression_method, 0 | 8) {
        return Err(LaunchPreparationError::new(
            LaunchPreparationFailureKind::UnsupportedArchive,
            format!(
                "unsupported ZIP compression method {}: {}::{}",
                member.compression_method, member.archive_path, member.member_path
            ),
        ));
    }
    let member_path = Path::new(&member.member_path);
    if member_path.is_absolute()
        || member_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(LaunchPreparationError::new(
            LaunchPreparationFailureKind::DamagedArchive,
            format!("unsafe ZIP member path: {}", member.member_path),
        ));
    }
    let file_name = member_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            LaunchPreparationError::new(
                LaunchPreparationFailureKind::UnsupportedArchive,
                format!(
                    "archive member has no usable file name: {}",
                    member.member_path
                ),
            )
        })?;

    let mut archive = File::open(&member.archive_path).map_err(|error| {
        let kind = if error.kind() == std::io::ErrorKind::NotFound {
            LaunchPreparationFailureKind::MissingPayload
        } else {
            LaunchPreparationFailureKind::UnreadablePayload
        };
        LaunchPreparationError::new(kind, format!("open {}: {error}", member.archive_path))
    })?;
    archive
        .seek(SeekFrom::Start(member.local_header_offset))
        .map_err(|error| damaged_archive(member, "seek local header", error))?;
    let mut header = [0u8; 30];
    archive
        .read_exact(&mut header)
        .map_err(|error| damaged_archive(member, "read local header", error))?;
    if u32::from_le_bytes(header[0..4].try_into().expect("ZIP signature bytes")) != 0x0403_4b50 {
        return Err(LaunchPreparationError::new(
            LaunchPreparationFailureKind::DamagedArchive,
            format!(
                "invalid ZIP local header: {}::{}",
                member.archive_path, member.member_path
            ),
        ));
    }
    let name_len = u16::from_le_bytes(header[26..28].try_into().expect("ZIP name length")) as u64;
    let extra_len = u16::from_le_bytes(header[28..30].try_into().expect("ZIP extra length")) as u64;
    archive
        .seek(SeekFrom::Current((name_len + extra_len) as i64))
        .map_err(|error| damaged_archive(member, "seek member payload", error))?;

    fs::create_dir_all(stage_dir).map_err(|error| {
        LaunchPreparationError::new(
            LaunchPreparationFailureKind::UnreadablePayload,
            format!("create archive staging {}: {error}", stage_dir.display()),
        )
    })?;
    let mut hasher = DefaultHasher::new();
    member.hash(&mut hasher);
    let output = stage_dir.join(format!("{:016x}-{file_name}", hasher.finish()));
    let partial = output.with_extension(format!(
        "{}.part",
        output
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("payload")
    ));
    let result = write_extracted_member(member, &mut archive, &partial);
    if let Err(error) = result {
        let _ = fs::remove_file(&partial);
        cleanup_archive_launch_staging_at(stage_dir);
        return Err(error);
    }
    fs::rename(&partial, &output).map_err(|error| {
        let _ = fs::remove_file(&partial);
        LaunchPreparationError::new(
            LaunchPreparationFailureKind::UnreadablePayload,
            format!(
                "publish staged archive member {}: {error}",
                output.display()
            ),
        )
    })?;
    Ok(output)
}

fn write_extracted_member(
    member: &mister_magik_catalog::archive_member::ArchiveMemberRef,
    archive: &mut File,
    output: &Path,
) -> Result<(), LaunchPreparationError> {
    let compressed = archive.take(member.compressed_size);
    let mut input: Box<dyn Read + '_> = match member.compression_method {
        0 => Box::new(compressed),
        8 => Box::new(DeflateDecoder::new(compressed)),
        _ => unreachable!("compression method validated"),
    };
    let mut output_file = File::create(output).map_err(|error| {
        LaunchPreparationError::new(
            LaunchPreparationFailureKind::UnreadablePayload,
            format!("create staged payload {}: {error}", output.display()),
        )
    })?;
    let mut crc = crc32fast::Hasher::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 32 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| damaged_archive(member, "decompress member", error))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > member.uncompressed_size || total > MAX_ARCHIVE_MEMBER_BYTES {
            return Err(LaunchPreparationError::new(
                LaunchPreparationFailureKind::OversizedArchiveMember,
                format!(
                    "archive member expanded beyond declared bounds: {}",
                    member.member_path
                ),
            ));
        }
        crc.update(&buffer[..read]);
        output_file.write_all(&buffer[..read]).map_err(|error| {
            LaunchPreparationError::new(
                LaunchPreparationFailureKind::UnreadablePayload,
                format!("write staged payload {}: {error}", output.display()),
            )
        })?;
    }
    if total != member.uncompressed_size || crc.finalize() != member.crc32 {
        return Err(LaunchPreparationError::new(
            LaunchPreparationFailureKind::DamagedArchive,
            format!(
                "ZIP member size or checksum mismatch: {}::{}",
                member.archive_path, member.member_path
            ),
        ));
    }
    output_file.sync_all().map_err(|error| {
        LaunchPreparationError::new(
            LaunchPreparationFailureKind::UnreadablePayload,
            format!("sync staged payload {}: {error}", output.display()),
        )
    })
}

fn damaged_archive(
    member: &mister_magik_catalog::archive_member::ArchiveMemberRef,
    operation: &str,
    error: std::io::Error,
) -> LaunchPreparationError {
    LaunchPreparationError::new(
        LaunchPreparationFailureKind::DamagedArchive,
        format!(
            "{operation} {}::{}: {error}",
            member.archive_path, member.member_path
        ),
    )
}

fn prepare_launch_ref_with_roots(launch_ref: &str, roots: &[String]) -> Result<String, String> {
    let mut fault_control = mister_magik_catalog::fs_fault::NoopDirectResetFaultControl;
    prepare_launch_ref_with_roots_and_fault_control(launch_ref, roots, &mut fault_control)
}

fn prepare_launch_ref_with_roots_and_fault_control(
    launch_ref: &str,
    roots: &[String],
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<String, String> {
    if launch_ref.starts_with(VIRTUAL_LAUNCH_PREFIX) {
        Err(format!(
            "structured launch ref must be resolved from catalog before launch: {launch_ref}"
        ))
    } else if launch_ref.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX) {
        materialize_amigavision_game_launch_ref(launch_ref, roots, fault_control)
    } else if launch_ref == AMIGAVISION_LAUNCHER_REF {
        materialize_amigavision_launcher_ref(roots)
    } else {
        Ok(launch_ref.to_string())
    }
}

fn materialize_amigavision_launcher_ref(roots: &[String]) -> Result<String, String> {
    let install = resolve_amigavision_install(roots)?;
    materialize_amigavision_launcher_ref_at(
        &install.mgl_path,
        &install.hdf_path,
        &install.shared_dir,
        &install.ags_boot_path,
    )
}

fn materialize_amigavision_game_launch_ref(
    launch_ref: &str,
    roots: &[String],
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<String, String> {
    let selection = launch_ref
        .strip_prefix(AMIGAVISION_GAME_LAUNCH_PREFIX)
        .ok_or_else(|| format!("invalid AmigaVision launch ref: {launch_ref}"))?;
    let (listing_kind, encoded) = selection
        .split_once(':')
        .ok_or_else(|| format!("invalid AmigaVision launch ref: {launch_ref}"))?;
    let title = decode_launch_component(encoded)?;
    let install = resolve_amigavision_install(roots)?;
    let listing = match listing_kind {
        "games" => &install.games_listing,
        "demos" => &install.demos_listing,
        _ => return Err(format!("invalid AmigaVision listing kind: {listing_kind}")),
    };
    if !listing_contains_exact(listing, &title)? {
        return Err(format!(
            "AmigaVision selection is no longer installed: {title}"
        ));
    }
    materialize_amigavision_game_launch_ref_at_with_fault_control(
        &title,
        &install.mgl_path,
        &install.hdf_path,
        &install.shared_dir,
        &install.ags_boot_path,
        fault_control,
    )
}

fn resolve_amigavision_install(roots: &[String]) -> Result<AmigaVisionInstall, String> {
    for storage_root in
        mister_magik_catalog::prepared_collections::storage_roots_for_library_roots(roots)
    {
        let amiga_dir = storage_root.join("games/Amiga");
        let games_listing = amiga_dir.join("listings/games.txt");
        let demos_listing = amiga_dir.join("listings/demos.txt");
        let shared_dir = amiga_dir.join("shared");
        if !games_listing.is_file() || !demos_listing.is_file() || !shared_dir.is_dir() {
            continue;
        }
        for hdf_name in AMIGAVISION_HDF_NAMES {
            let hdf_path = amiga_dir.join(hdf_name);
            if !hdf_path.is_file() {
                continue;
            }
            for mgl_name in AMIGAVISION_MGL_NAMES {
                let mgl_path = storage_root.join("_Computer").join(mgl_name);
                if validate_amigavision_install(&mgl_path, &hdf_path).is_ok() {
                    return Ok(AmigaVisionInstall {
                        mgl_path,
                        hdf_path,
                        ags_boot_path: shared_dir.join("ags_boot"),
                        shared_dir,
                        games_listing,
                        demos_listing,
                    });
                }
            }
        }
    }
    Err(
        "no complete AmigaVision or MegaAGS installation was found in configured library roots"
            .to_string(),
    )
}

fn listing_contains_exact(path: &Path, title: &str) -> Result<bool, String> {
    let bytes =
        fs::read(path).map_err(|e| format!("read AmigaVision listing {}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.lines().map(str::trim).any(|entry| entry == title))
}

fn materialize_amigavision_launcher_ref_at(
    mgl_path: &Path,
    hdf_path: &Path,
    shared_dir: &Path,
    ags_boot_path: &Path,
) -> Result<String, String> {
    validate_amigavision_install(mgl_path, hdf_path)?;
    if !shared_dir.is_dir() {
        return Err(format!(
            "AmigaVision shared directory is missing: {}",
            shared_dir.display()
        ));
    }
    match fs::remove_file(ags_boot_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove stale AmigaVision ags_boot: {e}")),
    }
    Ok(mgl_path.display().to_string())
}

#[cfg(test)]
fn materialize_amigavision_game_launch_ref_at(
    title: &str,
    mgl_path: &Path,
    hdf_path: &Path,
    shared_dir: &Path,
    ags_boot_path: &Path,
) -> Result<String, String> {
    let mut fault_control = mister_magik_catalog::fs_fault::NoopDirectResetFaultControl;
    materialize_amigavision_game_launch_ref_at_with_fault_control(
        title,
        mgl_path,
        hdf_path,
        shared_dir,
        ags_boot_path,
        &mut fault_control,
    )
}

fn materialize_amigavision_game_launch_ref_at_with_fault_control(
    title: &str,
    mgl_path: &Path,
    hdf_path: &Path,
    shared_dir: &Path,
    ags_boot_path: &Path,
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<String, String> {
    validate_amigavision_install(mgl_path, hdf_path)?;
    if !shared_dir.is_dir() {
        return Err(format!(
            "AmigaVision shared directory is missing: {}",
            shared_dir.display()
        ));
    }
    let content = format!("{title}\n");
    if fs::read_to_string(ags_boot_path)
        .map(|existing| existing == content)
        .unwrap_or(false)
    {
        record_descriptor_skipped();
    } else {
        write_descriptor_atomically(ags_boot_path, content.as_bytes(), fault_control)?;
        record_descriptor_written(content.len() as u64);
    }
    Ok(mgl_path.display().to_string())
}

fn write_descriptor_atomically(
    path: &Path,
    bytes: &[u8],
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("AmigaVision descriptor has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| {
        format!(
            "create AmigaVision descriptor parent {}: {e}",
            parent.display()
        )
    })?;
    let temp_path = descriptor_temp_path(path);
    let _ = fs::remove_file(&temp_path);
    let mut file =
        File::create(&temp_path).map_err(|e| format!("create AmigaVision descriptor temp: {e}"))?;
    file.write_all(bytes)
        .map_err(|e| format!("write AmigaVision descriptor temp: {e}"))?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "amigavision_descriptor.after_temp_write",
        path,
        fault_control,
    );
    file.sync_all()
        .map_err(|e| format!("sync AmigaVision descriptor temp: {e}"))?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "amigavision_descriptor.after_temp_sync",
        path,
        fault_control,
    );
    drop(file);
    fs::rename(&temp_path, path).map_err(|e| {
        let _ = fs::remove_file(&temp_path);
        format!(
            "rename AmigaVision descriptor {} -> {}: {e}",
            temp_path.display(),
            path.display()
        )
    })?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "amigavision_descriptor.after_rename_before_parent_sync",
        path,
        fault_control,
    );
    sync_path_best_effort(parent);
    Ok(())
}

fn sync_path_best_effort(path: &Path) {
    let _ = File::open(path).and_then(|file| file.sync_all());
}

fn descriptor_temp_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("ags_boot"),
        std::process::id()
    ))
}

#[cfg(any(feature = "bench-tools", test))]
fn reset_descriptor_stats() {
    DESCRIPTOR_WRITTEN.with(|value| value.set(0));
    DESCRIPTOR_SKIPPED.with(|value| value.set(0));
    DESCRIPTOR_BYTES.with(|value| value.set(0));
}

#[cfg(any(feature = "bench-tools", test))]
fn descriptor_stats_snapshot() -> LaunchPrepDescriptorStats {
    LaunchPrepDescriptorStats {
        written: DESCRIPTOR_WRITTEN.with(Cell::get),
        skipped: DESCRIPTOR_SKIPPED.with(Cell::get),
        bytes: DESCRIPTOR_BYTES.with(Cell::get),
    }
}

fn record_descriptor_written(bytes: u64) {
    DESCRIPTOR_WRITTEN.with(|value| value.set(value.get().saturating_add(1)));
    DESCRIPTOR_BYTES.with(|value| value.set(value.get().saturating_add(bytes)));
}

fn record_descriptor_skipped() {
    DESCRIPTOR_SKIPPED.with(|value| value.set(value.get().saturating_add(1)));
}

fn validate_amigavision_install(mgl_path: &Path, hdf_path: &Path) -> Result<(), String> {
    if !mgl_path.is_file() {
        return Err(format!(
            "AmigaVision launcher is not installed: {}",
            mgl_path.display()
        ));
    }
    let mgl = fs::read_to_string(mgl_path)
        .map_err(|e| format!("read AmigaVision launcher {}: {e}", mgl_path.display()))?;
    let normalized = mgl.to_ascii_lowercase();
    if !normalized.contains("<rbf") || !normalized.contains("minimig") {
        return Err(format!(
            "AmigaVision launcher does not target Minimig: {}",
            mgl_path.display()
        ));
    }
    if !hdf_path.is_file() {
        return Err(format!(
            "AmigaVision HDF is not installed: {}",
            hdf_path.display()
        ));
    }
    Ok(())
}

fn decode_launch_component(value: &str) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(value.len());
    let input = value.as_bytes();
    let mut idx = 0usize;
    while idx < input.len() {
        if input[idx] == b'%' {
            if idx + 2 >= input.len() {
                return Err("invalid percent escape in launch ref".to_string());
            }
            let hi = hex_value(input[idx + 1])
                .ok_or_else(|| "invalid percent escape in launch ref".to_string())?;
            let lo = hex_value(input[idx + 2])
                .ok_or_else(|| "invalid percent escape in launch ref".to_string())?;
            bytes.push((hi << 4) | lo);
            idx += 3;
        } else {
            bytes.push(input[idx]);
            idx += 1;
        }
    }
    String::from_utf8(bytes).map_err(|e| format!("invalid UTF-8 in launch ref: {e}"))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(feature = "bench-tools")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LaunchPrepBenchScenario {
    Warm,
    Cold,
    PriorityPrewarm,
}

#[cfg(feature = "bench-tools")]
impl LaunchPrepBenchScenario {
    fn from_arg(value: Option<&str>) -> Self {
        match value.unwrap_or("warm").trim().to_ascii_lowercase().as_str() {
            "cold" => Self::Cold,
            "priority-prewarm" | "prewarm" => Self::PriorityPrewarm,
            _ => Self::Warm,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Warm => "warm",
            Self::Cold => "cold",
            Self::PriorityPrewarm => "priority-prewarm",
        }
    }
}

#[cfg(feature = "bench-tools")]
#[derive(Clone, Debug)]
struct LaunchPrepBenchRef {
    kind: String,
    launch_ref: String,
}

#[cfg(any(feature = "bench-tools", all(test, not(target_os = "linux"))))]
#[derive(Clone, Copy, Debug, Default)]
struct ProcIoCounters {
    read_bytes: u64,
    rchar: u64,
    syscr: u64,
    write_bytes: u64,
    wchar: u64,
    syscw: u64,
}

#[cfg(feature = "bench-tools")]
pub fn run_launch_prep_bench() {
    let args: Vec<String> = std::env::args().collect();
    let label = std::env::var("MISTER_LAUNCH_PREP_LABEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| args.get(2).cloned())
        .unwrap_or_else(|| "launch-prep".to_string());
    let scenario = LaunchPrepBenchScenario::from_arg(args.get(3).map(String::as_str));
    let iterations = std::env::var("MISTER_LAUNCH_PREP_ITERATIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| args.get(4).and_then(|value| value.parse::<usize>().ok()))
        .unwrap_or(5)
        .max(1);
    let catalog = match library_db::load_arcade_catalog_from_sqlite(
        crate::arcade_catalog::DEFAULT_ARCADE_ROOT,
    ) {
        Ok(loaded) => loaded.catalog,
        Err(e) => {
            crate::ui_errln!("launch_prep_bench\tfailed\tload catalog: {e}");
            std::process::exit(1);
        }
    };
    let refs = match launch_prep_bench_refs_from_env()
        .or_else(|_| load_default_launch_prep_bench_refs(&catalog))
    {
        Ok(refs) => refs,
        Err(e) => {
            crate::ui_errln!("launch_prep_bench\tfailed\t{e}");
            std::process::exit(1);
        }
    };
    crate::ui_logln!(
        "launch_prep_bench label={label} scenario={} iterations={} refs={}",
        scenario.label(),
        iterations,
        refs.len()
    );
    if refs.is_empty() {
        crate::ui_logln!(
            "launch_prep_bench_summary\t{label}\t{}\tcount=0\terrors=0\tp50_us=0\tp95_us=0\tread_bytes=0\trchar=0\tsyscr=0\twrite_bytes=0\twchar=0\tsyscw=0",
            scenario.label()
        );
        return;
    }

    let mut samples = Vec::with_capacity(refs.len() * iterations);
    let mut errors = 0usize;
    let mut total_read_bytes = 0u64;
    let mut total_rchar = 0u64;
    let mut total_syscr = 0u64;
    let mut total_write_bytes = 0u64;
    let mut total_wchar = 0u64;
    let mut total_syscw = 0u64;
    let mut total_descriptor_written = 0u64;
    let mut total_descriptor_skipped = 0u64;
    let mut total_descriptor_bytes = 0u64;
    for iteration in 0..iterations {
        if scenario == LaunchPrepBenchScenario::PriorityPrewarm {
            let before = read_self_proc_io();
            let start = Instant::now();
            let prewarm_us = start.elapsed().as_micros() as u64;
            let after = read_self_proc_io();
            let read_bytes = after.read_bytes.saturating_sub(before.read_bytes);
            let rchar = after.rchar.saturating_sub(before.rchar);
            let syscr = after.syscr.saturating_sub(before.syscr);
            let write_bytes = after.write_bytes.saturating_sub(before.write_bytes);
            let wchar = after.wchar.saturating_sub(before.wchar);
            let syscw = after.syscw.saturating_sub(before.syscw);
            total_read_bytes = total_read_bytes.saturating_add(read_bytes);
            total_rchar = total_rchar.saturating_add(rchar);
            total_syscr = total_syscr.saturating_add(syscr);
            total_write_bytes = total_write_bytes.saturating_add(write_bytes);
            total_wchar = total_wchar.saturating_add(wchar);
            total_syscw = total_syscw.saturating_add(syscw);
            crate::ui_logln!(
                "launch_prep_bench_prewarm_tsv\t{label}\t{}\t{iteration}\tstatus=removed\ttotal=0\twritten=0\tunchanged=0\terrors=0\tprewarm_us={prewarm_us}\tread_bytes={read_bytes}\trchar={rchar}\tsyscr={syscr}\twrite_bytes={write_bytes}\twchar={wchar}\tsyscw={syscw}",
                scenario.label()
            );
        }
        for (idx, bench_ref) in refs.iter().enumerate() {
            if scenario == LaunchPrepBenchScenario::Cold {
                prepare_cold_launch_prep_ref(&bench_ref.launch_ref);
            }
            reset_descriptor_stats();
            let before = read_self_proc_io();
            let start = Instant::now();
            let result = prepare_launch_bench_ref(&catalog, &bench_ref.launch_ref);
            let prepare_us = start.elapsed().as_micros() as u64;
            let after = read_self_proc_io();
            let descriptor = descriptor_stats_snapshot();
            let read_bytes = after.read_bytes.saturating_sub(before.read_bytes);
            let rchar = after.rchar.saturating_sub(before.rchar);
            let syscr = after.syscr.saturating_sub(before.syscr);
            let write_bytes = after.write_bytes.saturating_sub(before.write_bytes);
            let wchar = after.wchar.saturating_sub(before.wchar);
            let syscw = after.syscw.saturating_sub(before.syscw);
            total_read_bytes = total_read_bytes.saturating_add(read_bytes);
            total_rchar = total_rchar.saturating_add(rchar);
            total_syscr = total_syscr.saturating_add(syscr);
            total_write_bytes = total_write_bytes.saturating_add(write_bytes);
            total_wchar = total_wchar.saturating_add(wchar);
            total_syscw = total_syscw.saturating_add(syscw);
            total_descriptor_written = total_descriptor_written.saturating_add(descriptor.written);
            total_descriptor_skipped = total_descriptor_skipped.saturating_add(descriptor.skipped);
            total_descriptor_bytes = total_descriptor_bytes.saturating_add(descriptor.bytes);
            let (status, target) = match result {
                Ok(target) => ("ok", target),
                Err(e) => {
                    errors += 1;
                    ("error", e)
                }
            };
            if status == "ok" {
                samples.push(prepare_us);
            }
            crate::ui_logln!(
                "launch_prep_bench_tsv\t{label}\t{}\t{iteration}\t{idx}\t{}\t{status}\t{prepare_us}\tread_bytes={read_bytes}\trchar={rchar}\tsyscr={syscr}\twrite_bytes={write_bytes}\twchar={wchar}\tsyscw={syscw}\tdescriptor_written={}\tdescriptor_skipped={}\tdescriptor_bytes={}\ttarget={}\tref={}",
                scenario.label(),
                bench_ref.kind,
                descriptor.written,
                descriptor.skipped,
                descriptor.bytes,
                target,
                bench_ref.launch_ref
            );
        }
    }
    samples.sort_unstable();
    let p50 = percentile_sample(&samples, 0.50);
    let p95 = percentile_sample(&samples, 0.95);
    crate::ui_logln!(
        "launch_prep_bench_summary\t{label}\t{}\tcount={}\terrors={errors}\tp50_us={p50}\tp95_us={p95}\tread_bytes={total_read_bytes}\trchar={total_rchar}\tsyscr={total_syscr}\twrite_bytes={total_write_bytes}\twchar={total_wchar}\tsyscw={total_syscw}\tdescriptor_written={total_descriptor_written}\tdescriptor_skipped={total_descriptor_skipped}\tdescriptor_bytes={total_descriptor_bytes}",
        scenario.label(),
        samples.len()
    );
}

#[cfg(feature = "bench-tools")]
fn prepare_launch_bench_ref(
    catalog: &crate::arcade_catalog::ArcadeCatalog,
    launch_ref: &str,
) -> Result<String, String> {
    if launch_ref.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX)
        || launch_ref == AMIGAVISION_LAUNCHER_REF
    {
        return prepare_launch_ref(launch_ref);
    }
    Ok(match catalog.launch_target_for_ref(launch_ref) {
        LaunchTarget::Path(path) => path.to_string(),
        LaunchTarget::Structured(plan) => format!("structured:{}", plan.launch_ref),
        LaunchTarget::Prepared(selection) => prepare_launch_ref(&selection.launch_ref)?,
        LaunchTarget::MissingStructured(launch_ref) => {
            return Err(format!(
                "structured launch plan missing from catalog: {launch_ref}"
            ));
        }
    })
}

#[cfg(feature = "bench-tools")]
fn launch_prep_bench_refs_from_env() -> Result<Vec<LaunchPrepBenchRef>, String> {
    let Ok(value) = std::env::var("MISTER_LAUNCH_PREP_REFS") else {
        return Err("MISTER_LAUNCH_PREP_REFS unset".to_string());
    };
    let refs = value
        .split('|')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|launch_ref| LaunchPrepBenchRef {
            kind: launch_prep_kind(launch_ref).to_string(),
            launch_ref: launch_ref.to_string(),
        })
        .collect();
    Ok(refs)
}

#[cfg(feature = "bench-tools")]
fn load_default_launch_prep_bench_refs(
    catalog: &crate::arcade_catalog::ArcadeCatalog,
) -> Result<Vec<LaunchPrepBenchRef>, String> {
    let virtual_limit = std::env::var("MISTER_LAUNCH_PREP_VIRTUAL_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(8);
    let amigavision_limit = std::env::var("MISTER_LAUNCH_PREP_AMIGAVISION_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(4);
    let mut refs = Vec::new();
    let virtual_systems = std::env::var("MISTER_LAUNCH_PREP_VIRTUAL_SYSTEMS")
        .unwrap_or_else(|_| "neogeo|saturn|snes".to_string());
    for system_id in virtual_systems
        .split('|')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        for game in catalog
            .system_game_view(system_id)
            .iter()
            .filter(|game| game.mra_path.starts_with(VIRTUAL_LAUNCH_PREFIX))
            .take(virtual_limit)
        {
            refs.push(LaunchPrepBenchRef {
                kind: format!("virtual-{}", game.system_id),
                launch_ref: game.mra_path.to_string(),
            });
        }
        if refs
            .iter()
            .any(|bench_ref| bench_ref.kind.starts_with("virtual-"))
        {
            break;
        }
    }
    for launch_ref in library_db::load_amigavision_launch_refs(amigavision_limit)? {
        refs.push(LaunchPrepBenchRef {
            kind: "amigavision".to_string(),
            launch_ref,
        });
    }
    Ok(refs)
}

#[cfg(any(feature = "bench-tools", test))]
fn launch_prep_kind(launch_ref: &str) -> &'static str {
    if launch_ref.starts_with(VIRTUAL_LAUNCH_PREFIX) {
        "virtual"
    } else if launch_ref.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX) {
        "amigavision"
    } else {
        "direct"
    }
}

#[cfg(feature = "bench-tools")]
fn prepare_cold_launch_prep_ref(launch_ref: &str) {
    if launch_ref.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX) {
        let roots = mister_magik_catalog::catalog_config::library_roots_from_env();
        if let Ok(install) = resolve_amigavision_install(&roots) {
            let _ = fs::remove_file(install.ags_boot_path);
        }
    }
}

#[cfg(any(feature = "bench-tools", all(test, not(target_os = "linux"))))]
fn read_self_proc_io() -> ProcIoCounters {
    let mut contents = String::new();
    if File::open("/proc/self/io")
        .and_then(|mut file| file.read_to_string(&mut contents))
        .is_err()
    {
        return ProcIoCounters::default();
    }
    let mut counters = ProcIoCounters::default();
    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("read_bytes:") {
            counters.read_bytes = value.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("rchar:") {
            counters.rchar = value.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("syscr:") {
            counters.syscr = value.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("write_bytes:") {
            counters.write_bytes = value.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("wchar:") {
            counters.wchar = value.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("syscw:") {
            counters.syscw = value.trim().parse::<u64>().unwrap_or(0);
        }
    }
    counters
}

#[cfg(any(feature = "bench-tools", test))]
fn percentile_sample(sorted: &[u64], percentile: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len().saturating_sub(1)) as f64 * percentile).ceil() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingFaultControl {
        points: Vec<String>,
    }

    impl mister_magik_catalog::fs_fault::DirectResetFaultControl for RecordingFaultControl {
        fn request_direct_reset(
            &mut self,
            request: &mister_magik_catalog::fs_fault::DirectResetFaultRequest,
        ) -> mister_magik_catalog::fs_fault::DirectResetFaultOutcome {
            self.points.push(request.point().to_string());
            mister_magik_catalog::fs_fault::DirectResetFaultOutcome::Noop
        }
    }

    fn minimig_mgl(hdf_name: &str) -> String {
        format!(
            "<mistergamedescription><rbf>_computer/minimig</rbf><file path=\"../games/Amiga/{hdf_name}\" index=\"0\"/></mistergamedescription>"
        )
    }

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("mister-magik-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    fn stored_zip_member(root: &Path, name: &str, payload: &[u8]) -> (PathBuf, u32) {
        let path = root.join("fixture.zip");
        let crc = crc32fast::hash(payload);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        bytes.extend_from_slice(&20u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(payload);
        std::fs::write(&path, bytes).expect("write ZIP fixture");
        (path, crc)
    }

    #[test]
    fn archive_member_is_extracted_to_bounded_temporary_staging() {
        let root = unique_temp_dir("archive-member");
        let stage = root.join("stage");
        let payload = b"acid-drop-rom";
        let (archive, crc32) = stored_zip_member(&root, "Acid Drop (Europe).bin", payload);
        let member = mister_magik_catalog::archive_member::ArchiveMemberRef {
            archive_path: archive.display().to_string(),
            member_path: "Acid Drop (Europe).bin".to_string(),
            local_header_offset: 0,
            compression_method: 0,
            compressed_size: payload.len() as u64,
            uncompressed_size: payload.len() as u64,
            crc32,
        };

        let extracted = extract_archive_member(&member, &stage).expect("extract archive member");
        assert!(extracted.starts_with(&stage));
        assert_eq!(
            std::fs::read(&extracted).expect("read extracted payload"),
            payload
        );
        cleanup_archive_launch_staging_at(&stage);
        assert!(!stage.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn archive_member_rejects_corrupt_unsupported_and_oversized_entries() {
        let root = unique_temp_dir("archive-member-errors");
        let stage = root.join("stage");
        let (archive, crc32) = stored_zip_member(&root, "Game.bin", b"rom");
        let mut member = mister_magik_catalog::archive_member::ArchiveMemberRef {
            archive_path: archive.display().to_string(),
            member_path: "Game.bin".to_string(),
            local_header_offset: 0,
            compression_method: 99,
            compressed_size: 3,
            uncompressed_size: 3,
            crc32,
        };
        assert_eq!(
            extract_archive_member(&member, &stage)
                .expect_err("unsupported method")
                .kind,
            LaunchPreparationFailureKind::UnsupportedArchive
        );
        member.compression_method = 0;
        member.uncompressed_size = MAX_ARCHIVE_MEMBER_BYTES + 1;
        assert_eq!(
            extract_archive_member(&member, &stage)
                .expect_err("oversized member")
                .kind,
            LaunchPreparationFailureKind::OversizedArchiveMember
        );
        member.uncompressed_size = 3;
        member.crc32 ^= 1;
        assert_eq!(
            extract_archive_member(&member, &stage)
                .expect_err("checksum mismatch")
                .kind,
            LaunchPreparationFailureKind::DamagedArchive
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn archive_member_classifies_missing_and_unsafe_payloads() {
        let root = unique_temp_dir("archive-member-classification");
        let stage = root.join("stage");
        let mut member = mister_magik_catalog::archive_member::ArchiveMemberRef {
            archive_path: root.join("missing.zip").display().to_string(),
            member_path: "Game.bin".to_string(),
            local_header_offset: 0,
            compression_method: 0,
            compressed_size: 3,
            uncompressed_size: 3,
            crc32: crc32fast::hash(b"rom"),
        };

        assert_eq!(
            extract_archive_member(&member, &stage)
                .expect_err("missing archive")
                .kind,
            LaunchPreparationFailureKind::MissingPayload
        );

        member.member_path = "../Game.bin".into();
        assert_eq!(
            extract_archive_member(&member, &stage)
                .expect_err("parent traversal")
                .kind,
            LaunchPreparationFailureKind::DamagedArchive
        );
        assert!(!stage.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn truncated_archive_payload_is_rejected_and_staging_is_cleaned() {
        let root = unique_temp_dir("archive-member-truncated");
        let stage = root.join("stage");
        let (archive, crc32) = stored_zip_member(&root, "Game.bin", b"rom");
        let member = mister_magik_catalog::archive_member::ArchiveMemberRef {
            archive_path: archive.display().to_string(),
            member_path: "Game.bin".to_string(),
            local_header_offset: 0,
            compression_method: 0,
            compressed_size: 2,
            uncompressed_size: 3,
            crc32,
        };

        assert_eq!(
            extract_archive_member(&member, &stage)
                .expect_err("truncated member")
                .kind,
            LaunchPreparationFailureKind::DamagedArchive
        );
        assert!(!stage.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn direct_launch_ref_prepares_without_materialization() {
        assert_eq!(
            prepare_launch_ref("/media/fat/_Arcade/test.mra").expect("prepare direct"),
            "/media/fat/_Arcade/test.mra"
        );
    }

    #[test]
    fn virtual_launch_ref_is_not_materialized_by_launch_prep() {
        let err = prepare_launch_ref("magik-plan:payload-saturn-test")
            .expect_err("virtual launch refs require catalog hydration");

        assert!(err.contains("structured launch ref must be resolved from catalog"));
    }

    #[test]
    fn amigavision_game_launch_ref_writes_ags_boot() {
        reset_descriptor_stats();
        let root = unique_temp_dir("amigavision-launch");
        let mgl = root.join("_Computer/Amiga.mgl");
        let hdf = root.join("games/Amiga/AmigaVision.hdf");
        let shared = root.join("games/Amiga/shared");
        let ags_boot = shared.join("ags_boot");
        std::fs::create_dir_all(mgl.parent().unwrap()).expect("create mgl dir");
        std::fs::create_dir_all(hdf.parent().unwrap()).expect("create hdf dir");
        std::fs::create_dir_all(&shared).expect("create shared dir");
        std::fs::write(&mgl, minimig_mgl("AmigaVision.hdf")).expect("write mgl");
        std::fs::write(&hdf, "hdf").expect("write hdf");

        let mut fault_control = RecordingFaultControl::default();
        let target = materialize_amigavision_game_launch_ref_at_with_fault_control(
            "4th & Inches (OCS)[en]",
            &mgl,
            &hdf,
            &shared,
            &ags_boot,
            &mut fault_control,
        )
        .expect("materialize AmigaVision game");

        assert_eq!(target, mgl.display().to_string());
        assert_eq!(
            std::fs::read_to_string(&ags_boot).expect("read ags_boot"),
            "4th & Inches (OCS)[en]\n"
        );
        assert_eq!(
            descriptor_stats_snapshot(),
            LaunchPrepDescriptorStats {
                written: 1,
                skipped: 0,
                bytes: "4th & Inches (OCS)[en]\n".len() as u64,
            }
        );
        assert_eq!(
            fault_control.points,
            vec![
                "amigavision_descriptor.after_temp_write",
                "amigavision_descriptor.after_temp_sync",
                "amigavision_descriptor.after_rename_before_parent_sync",
            ]
        );
        assert!(
            std::fs::read_dir(&shared)
                .expect("read shared dir")
                .flatten()
                .all(|entry| !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".ags_boot.tmp-")),
            "atomic temp should be renamed away"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_exact_title_validation_tolerates_unrelated_legacy_bytes() {
        let root = unique_temp_dir("amigavision-lossy-listing");
        let listing = root.join("games.txt");
        std::fs::write(&listing, b"1869 (AGA)[en]\nLegacy \xff title\n")
            .expect("write legacy listing");

        assert!(listing_contains_exact(&listing, "1869 (AGA)[en]").expect("validate exact title"));
        assert!(!listing_contains_exact(&listing, "Missing Game").expect("validate missing title"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_same_title_launch_skips_rewrite() {
        reset_descriptor_stats();
        let root = unique_temp_dir("amigavision-launch-same-title");
        let mgl = root.join("_Computer/Amiga.mgl");
        let hdf = root.join("games/Amiga/AmigaVision.hdf");
        let shared = root.join("games/Amiga/shared");
        let ags_boot = shared.join("ags_boot");
        std::fs::create_dir_all(mgl.parent().unwrap()).expect("create mgl dir");
        std::fs::create_dir_all(&shared).expect("create shared dir");
        std::fs::write(&mgl, minimig_mgl("AmigaVision.hdf")).expect("write mgl");
        std::fs::write(&hdf, "hdf").expect("write hdf");
        std::fs::write(&ags_boot, "Agony\n").expect("write ags_boot");
        let mut permissions = std::fs::metadata(&ags_boot)
            .expect("stat ags_boot")
            .permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&ags_boot, permissions).expect("make ags_boot read-only");

        let target =
            materialize_amigavision_game_launch_ref_at("Agony", &mgl, &hdf, &shared, &ags_boot)
                .expect("same-title launch should not rewrite ags_boot");

        assert_eq!(target, mgl.display().to_string());
        assert_eq!(
            std::fs::read_to_string(&ags_boot).expect("read ags_boot"),
            "Agony\n"
        );
        assert_eq!(
            descriptor_stats_snapshot(),
            LaunchPrepDescriptorStats {
                written: 0,
                skipped: 1,
                bytes: 0,
            }
        );
        let mut permissions = std::fs::metadata(&ags_boot)
            .expect("stat ags_boot")
            .permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&ags_boot, permissions).expect("restore ags_boot writable");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_launcher_ref_removes_stale_ags_boot() {
        let root = unique_temp_dir("amigavision-launcher");
        let mgl = root.join("_Computer/Amiga.mgl");
        let hdf = root.join("games/Amiga/AmigaVision.hdf");
        let shared = root.join("games/Amiga/shared");
        let ags_boot = shared.join("ags_boot");
        std::fs::create_dir_all(mgl.parent().unwrap()).expect("create mgl dir");
        std::fs::create_dir_all(&shared).expect("create shared dir");
        std::fs::write(&mgl, minimig_mgl("AmigaVision.hdf")).expect("write mgl");
        std::fs::write(&hdf, "hdf").expect("write hdf");
        std::fs::write(&ags_boot, "Agony\n").expect("write stale ags_boot");

        let target = materialize_amigavision_launcher_ref_at(&mgl, &hdf, &shared, &ags_boot)
            .expect("materialize AmigaVision launcher");

        assert_eq!(target, mgl.display().to_string());
        assert!(!ags_boot.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_launch_ref_reports_missing_hdf() {
        let root = unique_temp_dir("amigavision-missing-hdf");
        let mgl = root.join("_Computer/Amiga.mgl");
        let hdf = root.join("games/Amiga/AmigaVision.hdf");
        let shared = root.join("games/Amiga/shared");
        let ags_boot = shared.join("ags_boot");
        std::fs::create_dir_all(mgl.parent().unwrap()).expect("create mgl dir");
        std::fs::write(&mgl, minimig_mgl("AmigaVision.hdf")).expect("write mgl");

        let err =
            materialize_amigavision_game_launch_ref_at("Agony", &mgl, &hdf, &shared, &ags_boot)
                .expect_err("missing HDF should fail");

        assert!(err.contains("AmigaVision HDF is not installed"));
        let _ = std::fs::remove_dir_all(root);
    }

    fn write_complete_amigavision_install(
        root: &Path,
        hdf_name: &str,
        mgl_name: &str,
        games: &str,
        demos: &str,
    ) {
        let amiga = root.join("games/Amiga");
        std::fs::create_dir_all(root.join("_Computer")).expect("create computer dir");
        std::fs::create_dir_all(amiga.join("listings")).expect("create listings dir");
        std::fs::create_dir_all(amiga.join("shared")).expect("create shared dir");
        std::fs::write(root.join("_Computer").join(mgl_name), minimig_mgl(hdf_name))
            .expect("write launcher");
        std::fs::write(amiga.join(hdf_name), b"hdf").expect("write hdf");
        std::fs::write(amiga.join("listings/games.txt"), games).expect("write games listing");
        std::fs::write(amiga.join("listings/demos.txt"), demos).expect("write demos listing");
    }

    #[test]
    fn configured_roots_resolve_modern_install_and_validate_exact_title() {
        let root = unique_temp_dir("amigavision-dynamic-modern");
        write_complete_amigavision_install(
            &root,
            "AmigaVision.hdf",
            "Amiga.mgl",
            "Agony\nAlien Breed\n",
            "State of the Art\n",
        );
        let roots = vec![root.join("games").display().to_string()];

        let target = prepare_launch_ref_with_roots("magik-amigavision:games:Agony", &roots)
            .expect("prepare installed title");

        assert_eq!(
            target,
            root.join("_Computer/Amiga.mgl").display().to_string()
        );
        assert_eq!(
            std::fs::read_to_string(root.join("games/Amiga/shared/ags_boot"))
                .expect("read selector"),
            "Agony\n"
        );
        let err = prepare_launch_ref_with_roots("magik-amigavision:games:agony", &roots)
            .expect_err("title matching must remain exact");
        assert!(err.contains("no longer installed"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn configured_roots_support_legacy_install_names_and_demos() {
        let root = unique_temp_dir("amigavision-dynamic-legacy");
        write_complete_amigavision_install(
            &root,
            "MegaAGS.hdf",
            "Amiga 500.mgl",
            "Agony\n",
            "State of the Art\n",
        );
        let roots = vec![root.join("_Computer").display().to_string()];

        let target =
            prepare_launch_ref_with_roots("magik-amigavision:demos:State%20of%20the%20Art", &roots)
                .expect("prepare legacy demo");

        assert_eq!(
            target,
            root.join("_Computer/Amiga 500.mgl").display().to_string()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn configured_root_order_selects_one_install_deterministically() {
        let first = unique_temp_dir("amigavision-root-first");
        let second = unique_temp_dir("amigavision-root-second");
        write_complete_amigavision_install(&first, "AmigaVision.hdf", "Amiga.mgl", "Agony\n", "\n");
        write_complete_amigavision_install(
            &second,
            "AmigaVision.hdf",
            "Amiga.mgl",
            "Agony\n",
            "\n",
        );
        let roots = vec![
            second.join("games").display().to_string(),
            first.join("games").display().to_string(),
        ];

        let target = prepare_launch_ref_with_roots("magik-amigavision:games:Agony", &roots)
            .expect("prepare preferred root");

        assert_eq!(
            target,
            second.join("_Computer/Amiga.mgl").display().to_string()
        );
        assert!(!first.join("games/Amiga/shared/ags_boot").exists());
        let _ = std::fs::remove_dir_all(first);
        let _ = std::fs::remove_dir_all(second);
    }

    #[test]
    fn prepared_target_resolves_to_real_mgl_before_handoff() {
        let root = unique_temp_dir("amigavision-prepared-target");
        write_complete_amigavision_install(&root, "AmigaVision.hdf", "Amiga.mgl", "Agony\n", "\n");
        let target = LaunchTarget::Prepared(
            mister_magik_catalog::arcade_catalog::PreparedLaunchSelection {
                collection_id:
                    mister_magik_catalog::prepared_collections::PreparedCollectionId::AmigaVision,
                launch_ref: "magik-amigavision:games:Agony".into(),
            },
        );
        let roots = vec![root.join("games").display().to_string()];

        let prepared =
            prepare_launch_target_with_roots(&target, &roots).expect("prepare launch target");

        assert_eq!(
            prepared,
            LaunchTarget::Path(
                root.join("_Computer/Amiga.mgl")
                    .display()
                    .to_string()
                    .into()
            )
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn percent_decodes_amigavision_launch_title() {
        assert_eq!(
            decode_launch_component("4th%20%26%20Inches%20%28OCS%29%5Ben%5D")
                .expect("decode title"),
            "4th & Inches (OCS)[en]"
        );
    }

    #[test]
    fn percent_decode_rejects_bad_escapes_and_utf8() {
        assert_eq!(
            decode_launch_component("Agony%").expect_err("trailing percent"),
            "invalid percent escape in launch ref"
        );
        assert_eq!(
            decode_launch_component("Agony%XZ").expect_err("bad hex"),
            "invalid percent escape in launch ref"
        );
        assert!(
            decode_launch_component("%ff")
                .expect_err("bad utf8")
                .contains("invalid UTF-8 in launch ref")
        );
    }

    #[test]
    #[cfg(feature = "bench-tools")]
    fn launch_prep_bench_scenario_parses_aliases_and_defaults() {
        assert_eq!(
            LaunchPrepBenchScenario::from_arg(None),
            LaunchPrepBenchScenario::Warm
        );
        assert_eq!(
            LaunchPrepBenchScenario::from_arg(Some(" COLD ")),
            LaunchPrepBenchScenario::Cold
        );
        assert_eq!(
            LaunchPrepBenchScenario::from_arg(Some("prewarm")),
            LaunchPrepBenchScenario::PriorityPrewarm
        );
        assert_eq!(
            LaunchPrepBenchScenario::from_arg(Some("priority-prewarm")).label(),
            "priority-prewarm"
        );
        assert_eq!(
            LaunchPrepBenchScenario::from_arg(Some("surprise")),
            LaunchPrepBenchScenario::Warm
        );
    }

    #[test]
    fn launch_prep_kind_classifies_ref_families() {
        assert_eq!(launch_prep_kind("magik-plan:snes/foo"), "virtual");
        assert_eq!(launch_prep_kind("magik-amigavision:Agony"), "amigavision");
        assert_eq!(launch_prep_kind("/media/fat/_Arcade/foo.mra"), "direct");
    }

    #[test]
    fn percentile_sample_uses_upper_rank_and_handles_empty_samples() {
        assert_eq!(percentile_sample(&[], 0.95), 0);
        assert_eq!(percentile_sample(&[10], 0.95), 10);
        assert_eq!(percentile_sample(&[10, 20, 30, 40], 0.50), 30);
        assert_eq!(percentile_sample(&[10, 20, 30, 40], 0.95), 40);
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn proc_io_parser_returns_zeros_on_non_linux_hosts() {
        let counters = read_self_proc_io();
        assert_eq!(counters.read_bytes, 0);
        assert_eq!(counters.write_bytes, 0);
    }
}
