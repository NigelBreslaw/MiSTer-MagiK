// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::env;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn select_framebuffer_capture<T>(
    scanout_slots_present: bool,
    compatibility_mode: bool,
    read_latched: impl FnOnce() -> Result<T, String>,
    read_producer: impl FnOnce(&str) -> Result<T, String>,
    read_fb0: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if scanout_slots_present {
        match read_latched() {
            Ok(capture) => Ok(capture),
            Err(error) if compatibility_mode => read_producer(&error).map_err(|producer_error| {
                format!(
                    "authoritative scanout capture failed: {error}; producer composition capture failed: {producer_error}"
                )
            }),
            Err(error) => Err(format!("authoritative scanout capture failed: {error}")),
        }
    } else {
        read_fb0()
    }
}

#[cfg(any(target_os = "linux", test))]
use serde_json::{Value, json};

#[cfg(target_os = "linux")]
mod alpha_candidate;
#[cfg(target_os = "linux")]
mod launcher_automation;
#[cfg(any(target_os = "linux", test))]
mod runtime_upload;
#[cfg(target_os = "linux")]
mod scanout_slots_contract;
mod ui_test_runtime;

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, PartialEq)]
struct ControlRequest {
    id: Option<Value>,
    cmd: String,
    args: Value,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, PartialEq)]
struct ControlRequestError {
    id: Option<Value>,
    message: String,
}

#[cfg(any(target_os = "linux", test))]
fn parse_control_request(
    line: &str,
    token: &str,
    auth_disabled: bool,
) -> Result<ControlRequest, ControlRequestError> {
    let parsed: Value = serde_json::from_str(line.trim()).map_err(|error| ControlRequestError {
        id: None,
        message: format!("invalid json: {error}"),
    })?;
    let id = parsed.get("id").cloned();
    if !auth_disabled && parsed.get("token").and_then(Value::as_str) != Some(token) {
        return Err(ControlRequestError {
            id,
            message: "unauthorized".to_string(),
        });
    }
    let cmd = parsed
        .get("cmd")
        .and_then(Value::as_str)
        .ok_or_else(|| ControlRequestError {
            id: id.clone(),
            message: "missing cmd".to_string(),
        })?;
    Ok(ControlRequest {
        id,
        cmd: cmd.to_string(),
        args: parsed.get("args").cloned().unwrap_or_else(|| json!({})),
    })
}

#[cfg(any(target_os = "linux", test))]
fn require_ok_main_reply(reply: &str) -> Result<(), String> {
    if reply == "ok" || reply.starts_with("ok ") {
        Ok(())
    } else {
        Err(reply.to_string())
    }
}

#[cfg(any(target_os = "linux", test))]
struct IoOperationEvidence<'a> {
    operation: &'a str,
    request_received_monotonic_us: u64,
    operation_started_monotonic_us: u64,
    operation_ended_monotonic_us: u64,
    bytes_read: u64,
    bytes_written: u64,
    peak_buffer_ownership_bytes: u64,
    peak_rss_kb: Option<u64>,
    phases_us: Value,
}

#[cfg(any(target_os = "linux", test))]
fn io_operation_evidence(evidence: IoOperationEvidence<'_>) -> Value {
    let IoOperationEvidence {
        operation,
        request_received_monotonic_us,
        operation_started_monotonic_us,
        operation_ended_monotonic_us,
        bytes_read,
        bytes_written,
        peak_buffer_ownership_bytes,
        peak_rss_kb,
        phases_us,
    } = evidence;
    json!({
        "schema": "mister-magik-agent-io-operation-v1",
        "operation": operation,
        "clock_domain": "CLOCK_MONOTONIC",
        "request_received_monotonic_us": request_received_monotonic_us,
        "operation_started_monotonic_us": operation_started_monotonic_us,
        "operation_ended_monotonic_us": operation_ended_monotonic_us,
        "elapsed_us": operation_ended_monotonic_us.saturating_sub(operation_started_monotonic_us),
        "bytes_read": bytes_read,
        "bytes_written": bytes_written,
        "peak_buffer_ownership_bytes": peak_buffer_ownership_bytes,
        "peak_rss_kb": peak_rss_kb,
        "phases_us": phases_us,
    })
}

#[cfg(any(target_os = "linux", test))]
mod png_capture {
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;
    use std::time::Instant;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Geometry {
        pub width: usize,
        pub height: usize,
        pub stride: usize,
        pub bpp: usize,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Timing {
        pub rgba_convert_us: u64,
        pub zlib_encode_us: u64,
        pub crc32_us: u64,
        pub png_wrap_us: u64,
    }

    pub struct EncodeResult {
        pub bytes: Vec<u8>,
        pub timing: Timing,
        pub workspace_peak_bytes: u64,
    }

    pub fn encode(raw: &[u8], geometry: Geometry) -> Result<EncodeResult, String> {
        validate(raw, geometry)?;
        let (idat, rgba_convert_us, zlib_encode_us, stream_peak_bytes) =
            encode_scanlines(raw, geometry)?;

        let wrap_t = Instant::now();
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(
            &u32::try_from(geometry.width)
                .map_err(|_| "PNG width exceeds u32".to_string())?
                .to_be_bytes(),
        );
        ihdr.extend_from_slice(
            &u32::try_from(geometry.height)
                .map_err(|_| "PNG height exceeds u32".to_string())?
                .to_be_bytes(),
        );
        // Eight-bit PNG truecolour: the source has no useful alpha channel.
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let crc32_us = png_chunk(&mut png, b"IHDR", &ihdr)
            .saturating_add(png_chunk(&mut png, b"IDAT", &idat))
            .saturating_add(png_chunk(&mut png, b"IEND", &[]));
        let png_wrap_us = elapsed_us(wrap_t);
        let wrap_peak_bytes = (idat.len() as u64).saturating_add(png.len() as u64);

        Ok(EncodeResult {
            bytes: png,
            timing: Timing {
                rgba_convert_us,
                zlib_encode_us,
                crc32_us,
                png_wrap_us,
            },
            workspace_peak_bytes: stream_peak_bytes.max(wrap_peak_bytes),
        })
    }

    pub fn logical_rgba_len(geometry: Geometry) -> Result<usize, String> {
        geometry
            .width
            .checked_mul(4)
            .and_then(|bytes| bytes.checked_add(1))
            .and_then(|row| row.checked_mul(geometry.height))
            .ok_or_else(|| "PNG image size overflow".to_string())
    }

    fn validate(raw: &[u8], geometry: Geometry) -> Result<(), String> {
        let bytes_per_pixel = match geometry.bpp {
            16 => 2,
            32 => 4,
            _ => return Err(format!("unsupported framebuffer bpp {}", geometry.bpp)),
        };
        let active_row_bytes = geometry
            .width
            .checked_mul(bytes_per_pixel)
            .ok_or_else(|| "framebuffer row size overflow".to_string())?;
        if geometry.stride < active_row_bytes {
            return Err(format!(
                "framebuffer stride {} is smaller than active row {active_row_bytes}",
                geometry.stride
            ));
        }
        let expected = geometry
            .stride
            .checked_mul(geometry.height)
            .ok_or_else(|| "framebuffer byte size overflow".to_string())?;
        if raw.len() < expected {
            return Err(format!(
                "raw framebuffer has {} bytes, expected at least {expected}",
                raw.len()
            ));
        }
        let _ = geometry
            .width
            .checked_mul(3)
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or_else(|| "PNG scanline size overflow".to_string())?;
        Ok(())
    }

    fn encode_scanlines(
        raw: &[u8],
        geometry: Geometry,
    ) -> Result<(Vec<u8>, u64, u64, u64), String> {
        let row_capacity = geometry
            .width
            .checked_mul(3)
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or_else(|| "PNG scanline size overflow".to_string())?;
        let mut row = Vec::with_capacity(row_capacity);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        let mut convert_us = 0u64;
        let mut zlib_us = 0u64;
        let mut peak_bytes = 0u64;

        for y in 0..geometry.height {
            let convert_t = Instant::now();
            row.clear();
            row.push(0);
            let source_row = &raw[y * geometry.stride..(y + 1) * geometry.stride];
            match geometry.bpp {
                16 => {
                    for pixel in source_row[..geometry.width * 2].as_chunks::<2>().0 {
                        let value = u16::from_le_bytes([pixel[0], pixel[1]]);
                        let r5 = (value >> 11) & 0x1f;
                        let g6 = (value >> 5) & 0x3f;
                        let b5 = value & 0x1f;
                        row.extend_from_slice(&[
                            ((r5 << 3) | (r5 >> 2)) as u8,
                            ((g6 << 2) | (g6 >> 4)) as u8,
                            ((b5 << 3) | (b5 >> 2)) as u8,
                        ]);
                    }
                }
                32 => {
                    for pixel in source_row[..geometry.width * 4].as_chunks::<4>().0 {
                        row.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
                    }
                }
                _ => unreachable!("geometry was validated"),
            }
            convert_us = convert_us.saturating_add(elapsed_us(convert_t));

            let zlib_t = Instant::now();
            encoder.write_all(&row).map_err(|error| error.to_string())?;
            zlib_us = zlib_us.saturating_add(elapsed_us(zlib_t));
            peak_bytes =
                peak_bytes.max((row.len() as u64).saturating_add(encoder.get_ref().len() as u64));
        }

        let finish_t = Instant::now();
        let idat = encoder.finish().map_err(|error| error.to_string())?;
        zlib_us = zlib_us.saturating_add(elapsed_us(finish_t));
        peak_bytes = peak_bytes.max(idat.len() as u64);
        Ok((idat, convert_us, zlib_us, peak_bytes))
    }

    fn png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) -> u64 {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(tag);
        out.extend_from_slice(data);
        let crc_start = Instant::now();
        let crc = crc32_extend(crc32_extend(0xffff_ffff, tag), data);
        out.extend_from_slice(&(!crc).to_be_bytes());
        elapsed_us(crc_start)
    }

    fn crc32_extend(mut crc: u32, data: &[u8]) -> u32 {
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = 0u32.wrapping_sub(crc & 1);
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        crc
    }

    fn elapsed_us(start: Instant) -> u64 {
        start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }
}

#[cfg(any(target_os = "linux", test))]
#[cfg_attr(all(test, not(target_os = "linux")), allow(dead_code))]
mod sd_browse {
    use quick_xml::Reader as XmlReader;
    use quick_xml::XmlVersion;
    use quick_xml::events::{BytesStart, Event};
    use serde_json::{Value, json};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Instant, UNIX_EPOCH};

    pub const ROOT_PATH: &str = "/";
    pub const SD_ROOT: &str = "/media/fat";
    pub const MRA_PARSE_LIMIT_BYTES: u64 = 512 * 1024;
    pub const MRA_RAW_DISPLAY_LIMIT_BYTES: u64 = 256 * 1024;
    pub const IMAGE_PREVIEW_LIMIT_BYTES: u64 = 16 * 1024 * 1024;

    pub fn list_dir_at_root(
        root: &Path,
        requested_path: &str,
        show_hidden: bool,
    ) -> Result<Value, String> {
        let start = Instant::now();
        let relative_path = normalize_sd_relative_path(requested_path)?;
        let host_path = checked_sd_host_path(root, &relative_path)
            .map_err(|err| format!("read_dir {relative_path}: {err}"))?;
        let enumerate_start = Instant::now();
        let mut entries = Vec::new();
        for entry in
            fs::read_dir(&host_path).map_err(|err| format!("read_dir {relative_path}: {err}"))?
        {
            let entry = entry.map_err(|err| format!("read_dir {relative_path}: {err}"))?;
            if !show_hidden && is_hidden_name(&entry.file_name().to_string_lossy()) {
                continue;
            }
            entries.push(sd_entry_json(&relative_path, entry)?);
        }
        let enumerate_us = enumerate_start.elapsed().as_micros() as u64;
        let sort_start = Instant::now();
        entries.sort_by(sd_entry_value_cmp);
        let sort_us = sort_start.elapsed().as_micros() as u64;
        let serialization_start = Instant::now();
        let serialized_bytes = serde_json::to_vec(&entries)
            .map_err(|err| format!("serialize directory entries: {err}"))?
            .len() as u64;
        let serialization_us = serialization_start.elapsed().as_micros() as u64;
        let entry_count = entries.len() as u64;
        Ok(json!({
            "schema": "mister-magik-sd-list-dir-v1",
            "path": relative_path,
            "show_hidden": show_hidden,
            "entries": entries,
            "elapsed_ms": start.elapsed().as_millis() as u64,
            "io_phases_us": {
                "directory_enumeration": enumerate_us,
                "sort": sort_us,
                "serialization": serialization_us,
            },
            "io_counts": {
                "entries": entry_count,
                "serialized_bytes": serialized_bytes,
            },
        }))
    }

    pub fn list_dir_fast_at_root(
        root: &Path,
        requested_path: &str,
        show_hidden: bool,
    ) -> Result<Value, String> {
        let start = Instant::now();
        let relative_path = normalize_sd_relative_path(requested_path)?;
        let host_path = checked_sd_host_path(root, &relative_path)
            .map_err(|err| format!("read_dir {relative_path}: {err}"))?;
        let enumerate_start = Instant::now();
        let mut entries = Vec::new();
        for entry in
            fs::read_dir(&host_path).map_err(|err| format!("read_dir {relative_path}: {err}"))?
        {
            let entry = entry.map_err(|err| format!("read_dir {relative_path}: {err}"))?;
            if !show_hidden && is_hidden_name(&entry.file_name().to_string_lossy()) {
                continue;
            }
            entries.push(sd_entry_fast_json(&relative_path, entry)?);
        }
        let enumerate_us = enumerate_start.elapsed().as_micros() as u64;
        let sort_start = Instant::now();
        entries.sort_by(sd_entry_value_cmp);
        let sort_us = sort_start.elapsed().as_micros() as u64;
        let serialization_start = Instant::now();
        let serialized_bytes = serde_json::to_vec(&entries)
            .map_err(|err| format!("serialize directory entries: {err}"))?
            .len() as u64;
        let serialization_us = serialization_start.elapsed().as_micros() as u64;
        let entry_count = entries.len() as u64;
        Ok(json!({
            "schema": "mister-magik-sd-list-dir-v2",
            "path": relative_path,
            "show_hidden": show_hidden,
            "entries": entries,
            "elapsed_ms": start.elapsed().as_millis() as u64,
            "io_phases_us": {
                "directory_enumeration": enumerate_us,
                "sort": sort_us,
                "serialization": serialization_us,
            },
            "io_counts": {
                "entries": entry_count,
                "serialized_bytes": serialized_bytes,
            },
        }))
    }

    pub fn stat_item_at_root(root: &Path, requested_path: &str) -> Result<Value, String> {
        let start = Instant::now();
        let relative_path = normalize_sd_relative_path(requested_path)?;
        let host_path = checked_sd_host_path(root, &relative_path)
            .map_err(|err| format!("stat {relative_path}: {err}"))?;
        let metadata =
            fs::metadata(&host_path).map_err(|err| format!("stat {relative_path}: {err}"))?;
        let name = item_name(&relative_path);
        let extension = file_extension(&name);
        let kind = if metadata.is_dir() {
            "directory"
        } else {
            "file"
        };
        Ok(json!({
            "schema": "mister-magik-sd-stat-item-v1",
            "path": relative_path,
            "name": name,
            "parent_path": parent_sd_path(requested_path),
            "kind": kind,
            "size": if metadata.is_dir() { 0 } else { metadata.len() },
            "modified_unix_ms": modified_unix_ms(&metadata),
            "readonly": metadata.permissions().readonly(),
            "hidden": is_hidden_name(&name),
            "extension": extension,
            "capabilities": item_capabilities(kind, &extension),
            "elapsed_ms": start.elapsed().as_millis() as u64,
        }))
    }

    pub struct SdPreviewImage {
        pub result: Value,
        pub payload: Vec<u8>,
    }

    pub fn preview_image_at_root(
        root: &Path,
        requested_path: &str,
    ) -> Result<SdPreviewImage, String> {
        let start = Instant::now();
        let relative_path = normalize_sd_relative_path(requested_path)?;
        let host_path = checked_sd_host_path(root, &relative_path)
            .map_err(|err| format!("stat {relative_path}: {err}"))?;
        let metadata =
            fs::metadata(&host_path).map_err(|err| format!("stat {relative_path}: {err}"))?;
        if !metadata.is_file() {
            return Err(format!("preview target is not a file: {relative_path}"));
        }
        if metadata.len() > IMAGE_PREVIEW_LIMIT_BYTES {
            return Err(format!(
                "image {} bytes exceeds preview limit {}",
                metadata.len(),
                IMAGE_PREVIEW_LIMIT_BYTES
            ));
        }
        let extension = file_extension(&item_name(&relative_path));
        if !matches!(extension.as_str(), "png" | "jpg" | "jpeg") {
            return Err(format!("unsupported preview extension: {extension}"));
        }
        let payload = fs::read(&host_path).map_err(|err| format!("read {relative_path}: {err}"))?;
        let (format, width, height) = image_dimensions(&payload)
            .ok_or_else(|| "could not identify PNG/JPEG dimensions".to_string())?;
        Ok(SdPreviewImage {
            result: json!({
                "schema": "mister-magik-sd-preview-image-v1",
                "path": relative_path,
                "format": format,
                "width": width,
                "height": height,
                "raw_bytes": payload.len() as u64,
                "payload_bytes": payload.len() as u64,
                "encoding": "identity",
                "elapsed_ms": start.elapsed().as_millis() as u64,
            }),
            payload,
        })
    }

    pub fn parse_mra_at_root(root: &Path, requested_path: &str) -> Result<Value, String> {
        let start = Instant::now();
        let relative_path = normalize_sd_relative_path(requested_path)?;
        let host_path = checked_sd_host_path(root, &relative_path)
            .map_err(|err| format!("stat {relative_path}: {err}"))?;
        let metadata =
            fs::metadata(&host_path).map_err(|err| format!("stat {relative_path}: {err}"))?;
        if !metadata.is_file() {
            return Err(format!("MRA target is not a file: {relative_path}"));
        }
        if file_extension(&item_name(&relative_path)) != "mra" {
            return Err(format!(
                "MRA parser only accepts .mra files: {relative_path}"
            ));
        }
        if metadata.len() > MRA_PARSE_LIMIT_BYTES {
            return Ok(json!({
                "schema": "mister-magik-sd-parse-mra-v1",
                "path": relative_path,
                "size": metadata.len(),
                "parse_limit_bytes": MRA_PARSE_LIMIT_BYTES,
                "raw_display_limit_bytes": MRA_RAW_DISPLAY_LIMIT_BYTES,
                "truncated": true,
                "summary": [],
                "xml_rows": [],
                "path_rows": [],
                "warnings": [format!("MRA is {} bytes; parse limit is {}", metadata.len(), MRA_PARSE_LIMIT_BYTES)],
                "raw_xml": "",
                "raw_xml_truncated": true,
                "elapsed_ms": start.elapsed().as_millis() as u64,
            }));
        }
        let text =
            fs::read_to_string(&host_path).map_err(|err| format!("read {relative_path}: {err}"))?;
        let parsed = parse_mra_text(&text, metadata.len());
        Ok(json!({
            "schema": "mister-magik-sd-parse-mra-v1",
            "path": relative_path,
            "size": metadata.len(),
            "parse_limit_bytes": MRA_PARSE_LIMIT_BYTES,
            "raw_display_limit_bytes": MRA_RAW_DISPLAY_LIMIT_BYTES,
            "truncated": false,
            "summary": parsed.summary,
            "xml_rows": parsed.xml_rows,
            "path_rows": parsed.path_rows,
            "warnings": parsed.warnings,
            "raw_xml": if metadata.len() <= MRA_RAW_DISPLAY_LIMIT_BYTES { text } else { String::new() },
            "raw_xml_truncated": metadata.len() > MRA_RAW_DISPLAY_LIMIT_BYTES,
            "elapsed_ms": start.elapsed().as_millis() as u64,
        }))
    }

    pub fn normalize_sd_relative_path(path: &str) -> Result<String, String> {
        let trimmed = path.trim();
        if trimmed.is_empty() || trimmed == ROOT_PATH {
            return Ok(ROOT_PATH.to_string());
        }
        if trimmed.starts_with("/media/fat/") || trimmed == SD_ROOT {
            return Err("sd path must be relative to /media/fat".to_string());
        }
        let mut parts = Vec::new();
        for part in trimmed.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." {
                return Err("sd path may not contain ..".to_string());
            }
            if part.contains('\0') {
                return Err("sd path may not contain NUL".to_string());
            }
            parts.push(part);
        }
        if parts.is_empty() {
            Ok(ROOT_PATH.to_string())
        } else {
            Ok(format!("/{}", parts.join("/")))
        }
    }

    pub fn sd_host_path(root: &Path, relative_path: &str) -> PathBuf {
        let mut path = root.to_path_buf();
        for part in relative_path.split('/').filter(|part| !part.is_empty()) {
            path.push(part);
        }
        path
    }

    fn checked_sd_host_path(root: &Path, relative_path: &str) -> Result<PathBuf, String> {
        let canonical_root = fs::canonicalize(root)
            .map_err(|err| format!("resolve SD root {}: {err}", root.display()))?;
        let host_path = sd_host_path(root, relative_path);
        let canonical_path = fs::canonicalize(&host_path)
            .map_err(|err| format!("resolve {relative_path}: {err}"))?;
        if !canonical_path.starts_with(&canonical_root) {
            return Err(format!(
                "resolved path is outside SD root: {}",
                canonical_path.display()
            ));
        }
        Ok(canonical_path)
    }

    pub fn sd_entry_json(parent_path: &str, entry: fs::DirEntry) -> Result<Value, String> {
        let name = entry.file_name().to_string_lossy().to_string();
        let entry_path = child_sd_path(parent_path, &name);
        let file_type = entry
            .file_type()
            .map_err(|err| format!("file_type {entry_path}: {err}"))?;
        let metadata = entry
            .metadata()
            .map_err(|err| format!("metadata {entry_path}: {err}"))?;
        let kind = if file_type.is_dir() {
            "directory"
        } else {
            "file"
        };
        let modified_unix_ms = modified_unix_ms(&metadata);
        Ok(json!({
            "name": name,
            "path": entry_path,
            "kind": kind,
            "size": if file_type.is_dir() { 0 } else { metadata.len() },
            "modified_unix_ms": modified_unix_ms,
            "readonly": metadata.permissions().readonly(),
            "hidden": is_hidden_name(&name),
        }))
    }

    fn sd_entry_fast_json(parent_path: &str, entry: fs::DirEntry) -> Result<Value, String> {
        let name = entry.file_name().to_string_lossy().to_string();
        let entry_path = child_sd_path(parent_path, &name);
        let file_type = entry
            .file_type()
            .map_err(|err| format!("file_type {entry_path}: {err}"))?;
        Ok(json!({
            "name": name,
            "path": entry_path,
            "kind": if file_type.is_dir() { "directory" } else { "file" },
        }))
    }

    fn is_hidden_name(name: &str) -> bool {
        name.starts_with('.')
    }

    fn modified_unix_ms(metadata: &fs::Metadata) -> u64 {
        metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
    }

    fn item_name(path: &str) -> String {
        if path == ROOT_PATH {
            "SD Card".to_string()
        } else {
            path.rsplit('/').next().unwrap_or(path).to_string()
        }
    }

    fn parent_sd_path(path: &str) -> String {
        let normalized = normalize_sd_relative_path(path).unwrap_or_else(|_| ROOT_PATH.to_string());
        if normalized == ROOT_PATH {
            return ROOT_PATH.to_string();
        }
        match normalized.rsplit_once('/') {
            Some(("", _)) | None => ROOT_PATH.to_string(),
            Some((parent, _)) => parent.to_string(),
        }
    }

    fn file_extension(name: &str) -> String {
        name.rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .unwrap_or_default()
    }

    fn item_capabilities(kind: &str, extension: &str) -> Value {
        json!({
            "stat": true,
            "image_preview": kind == "file" && matches!(extension, "png" | "jpg" | "jpeg"),
            "mra_parse": kind == "file" && extension == "mra",
            "raw_xml": kind == "file" && extension == "mra",
            "folder_analysis": kind == "directory",
            "ini_summary": kind == "file" && extension == "ini",
            "rbf_summary": kind == "file" && extension == "rbf",
            "save_hint": kind == "file" && matches!(extension, "sav" | "srm"),
            "archive_summary": kind == "file" && matches!(extension, "zip" | "7z"),
            "sqlite_summary": kind == "file" && matches!(extension, "sqlite" | "sqlite3" | "db"),
        })
    }

    pub(super) fn image_dimensions(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
        if bytes.len() >= 24
            && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
            && bytes[8..12] == 13u32.to_be_bytes()
            && &bytes[12..16] == b"IHDR"
        {
            let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
            let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
            if width != 0 && height != 0 {
                return Some(("png", width, height));
            }
            return None;
        }
        if bytes.len() >= 4 && bytes[0..2] == [0xff, 0xd8] {
            let mut i = 2usize;
            while i + 9 < bytes.len() {
                while i < bytes.len() && bytes[i] == 0xff {
                    i += 1;
                }
                if i >= bytes.len() {
                    break;
                }
                let marker = bytes[i];
                i += 1;
                if marker == 0xd9 || marker == 0xda {
                    break;
                }
                if i + 2 > bytes.len() {
                    break;
                }
                let len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
                if len < 2 || i + len > bytes.len() {
                    break;
                }
                if matches!(
                    marker,
                    0xc0 | 0xc1
                        | 0xc2
                        | 0xc3
                        | 0xc5
                        | 0xc6
                        | 0xc7
                        | 0xc9
                        | 0xca
                        | 0xcb
                        | 0xcd
                        | 0xce
                        | 0xcf
                ) && len >= 8
                {
                    let height = u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]) as u32;
                    let width = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                    if width != 0 && height != 0 {
                        return Some(("jpeg", width, height));
                    }
                    return None;
                }
                i += len;
            }
        }
        None
    }

    struct ParsedMra {
        summary: Vec<Value>,
        xml_rows: Vec<Value>,
        path_rows: Vec<Value>,
        warnings: Vec<String>,
    }

    fn parse_mra_text(text: &str, source_size: u64) -> ParsedMra {
        let mut reader = XmlReader::from_str(text);
        reader.config_mut().trim_text(true);
        let mut stack: Vec<String> = Vec::new();
        let mut xml_rows = Vec::new();
        let mut warnings = Vec::new();
        let mut order = 0u64;

        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => {
                    push_start_row(&mut xml_rows, &mut stack, &e, &mut order, false);
                    stack.push(xml_name(e.name().as_ref()));
                }
                Ok(Event::Empty(e)) => {
                    push_start_row(&mut xml_rows, &mut stack, &e, &mut order, true);
                }
                Ok(Event::Text(e)) => {
                    let text_value = e.xml10_content().trim().to_string();
                    if !text_value.is_empty() {
                        let path = format!("/{}", stack.join("/"));
                        xml_rows.push(json!({
                            "order": order,
                            "depth": stack.len(),
                            "path": path,
                            "kind": "text",
                            "name": "",
                            "value": text_value,
                        }));
                        order += 1;
                    }
                }
                Ok(Event::End(_)) => {
                    stack.pop();
                }
                Ok(Event::Eof) => break,
                Err(err) => {
                    warnings.push(format!("XML parse warning: {err}"));
                    break;
                }
                _ => {}
            }
        }

        let summary = mra_summary_rows(&xml_rows, source_size);
        let path_rows = xml_rows
            .iter()
            .filter(|row| {
                let value = row.get("value").and_then(Value::as_str).unwrap_or("");
                looks_path_like(row.get("name").and_then(Value::as_str).unwrap_or(""))
                    || looks_path_like(value)
            })
            .cloned()
            .collect::<Vec<_>>();
        ParsedMra {
            summary,
            xml_rows,
            path_rows,
            warnings,
        }
    }

    fn push_start_row(
        rows: &mut Vec<Value>,
        stack: &mut [String],
        e: &BytesStart<'_>,
        order: &mut u64,
        empty: bool,
    ) {
        let name = xml_name(e.name().as_ref());
        let path = if stack.is_empty() {
            format!("/{name}")
        } else {
            format!("/{}/{}", stack.join("/"), name)
        };
        rows.push(json!({
            "order": *order,
            "depth": stack.len() + 1,
            "path": path,
            "kind": if empty { "empty-element" } else { "element" },
            "name": name,
            "value": "",
        }));
        *order += 1;
        for attr in e.attributes().flatten() {
            rows.push(json!({
                "order": *order,
                "depth": stack.len() + 1,
                "path": path,
                "kind": "attribute",
                "name": format!("@{}", xml_name(attr.key.as_ref())),
                "value": attr
                    .normalized_value(XmlVersion::Implicit1_0)
                    .unwrap_or_default()
                    .into_owned(),
            }));
            *order += 1;
        }
    }

    fn xml_name(name: &str) -> String {
        name.to_string()
    }

    fn mra_summary_rows(rows: &[Value], source_size: u64) -> Vec<Value> {
        let mut out = vec![json!({"label": "MRA size", "value": format!("{source_size} bytes")})];
        for (label, names) in [
            ("Title", &["name", "title"][..]),
            ("Set", &["setname", "set", "rom"][..]),
            ("Year", &["year"][..]),
            ("Manufacturer", &["manufacturer", "maker"][..]),
            ("Core/RBF", &["rbf", "core"][..]),
            ("Rotation", &["rotation", "rotate"][..]),
            ("Buttons", &["buttons"][..]),
        ] {
            if let Some(value) = first_named_value(rows, names) {
                out.push(json!({"label": label, "value": value}));
            }
        }
        out
    }

    fn first_named_value(rows: &[Value], names: &[&str]) -> Option<String> {
        rows.iter().find_map(|row| {
            let kind = row.get("kind").and_then(Value::as_str).unwrap_or("");
            let name = row.get("name").and_then(Value::as_str).unwrap_or("");
            let path_name = row
                .get("path")
                .and_then(Value::as_str)
                .and_then(|path| path.rsplit('/').next())
                .unwrap_or("");
            let key = name.trim_start_matches('@').to_ascii_lowercase();
            let path_key = path_name.to_ascii_lowercase();
            if (kind == "attribute" && names.contains(&key.as_str()))
                || (kind == "text" && names.contains(&path_key.as_str()))
            {
                let value = row
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
            None
        })
    }

    fn looks_path_like(value: &str) -> bool {
        let lowered = value.to_ascii_lowercase();
        lowered.contains('/')
            || lowered.ends_with(".rbf")
            || lowered.ends_with(".rom")
            || lowered.ends_with(".zip")
            || lowered.ends_with(".bin")
            || lowered.ends_with(".mra")
    }

    pub fn child_sd_path(parent_path: &str, name: &str) -> String {
        if parent_path == ROOT_PATH {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        }
    }

    pub fn sd_entry_value_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
        let a_dir = a.get("kind").and_then(Value::as_str) == Some("directory");
        let b_dir = b.get("kind").and_then(Value::as_str) == Some("directory");
        b_dir
            .cmp(&a_dir)
            .then_with(|| natural_name_cmp(entry_name(a), entry_name(b)))
            .then_with(|| entry_name(a).cmp(entry_name(b)))
    }

    fn entry_name(value: &Value) -> &str {
        value.get("name").and_then(Value::as_str).unwrap_or("")
    }

    fn natural_name_cmp(a: &str, b: &str) -> std::cmp::Ordering {
        let mut a_chars = a.char_indices().peekable();
        let mut b_chars = b.char_indices().peekable();
        loop {
            match (a_chars.peek().copied(), b_chars.peek().copied()) {
                (None, None) => return std::cmp::Ordering::Equal,
                (None, Some(_)) => return std::cmp::Ordering::Less,
                (Some(_), None) => return std::cmp::Ordering::Greater,
                (Some((_, ac)), Some((_, bc))) if ac.is_ascii_digit() && bc.is_ascii_digit() => {
                    let a_digits = take_ascii_digits(a, &mut a_chars);
                    let b_digits = take_ascii_digits(b, &mut b_chars);
                    let a_trimmed = a_digits.trim_start_matches('0');
                    let b_trimmed = b_digits.trim_start_matches('0');
                    let a_number = if a_trimmed.is_empty() { "0" } else { a_trimmed };
                    let b_number = if b_trimmed.is_empty() { "0" } else { b_trimmed };
                    let by_len = a_number.len().cmp(&b_number.len());
                    if by_len != std::cmp::Ordering::Equal {
                        return by_len;
                    }
                    let by_value = a_number.cmp(b_number);
                    if by_value != std::cmp::Ordering::Equal {
                        return by_value;
                    }
                    let by_raw_len = a_digits.len().cmp(&b_digits.len());
                    if by_raw_len != std::cmp::Ordering::Equal {
                        return by_raw_len;
                    }
                }
                (Some((_, ac)), Some((_, bc))) => {
                    a_chars.next();
                    b_chars.next();
                    let by_char = ac.to_ascii_lowercase().cmp(&bc.to_ascii_lowercase());
                    if by_char != std::cmp::Ordering::Equal {
                        return by_char;
                    }
                }
            }
        }
    }

    fn take_ascii_digits<'a>(
        text: &'a str,
        chars: &mut std::iter::Peekable<std::str::CharIndices<'a>>,
    ) -> &'a str {
        let start = chars.peek().map(|(index, _)| *index).unwrap_or(text.len());
        let mut end = start;
        while let Some((index, ch)) = chars.peek().copied() {
            if !ch.is_ascii_digit() {
                break;
            }
            end = index + ch.len_utf8();
            chars.next();
        }
        &text[start..end]
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::png_capture;
    use super::scanout_slots_contract::{
        DEVICE as SCANOUT_SLOTS_DEVICE, EXPECTED_LAYOUT, GET_LAYOUT as SCANOUT_SLOTS_GET_LAYOUT,
        ScanoutSlotsLayout,
    };
    use super::{
        ControlRequest, IoOperationEvidence, io_operation_evidence, parse_control_request,
        require_ok_main_reply, select_framebuffer_capture,
    };
    use libc::{c_ulong, ioctl};
    use mister_magik_framebuffer_stream::{
        FLAG_LZ4_SIZE_PREPENDED, FrameKind, MAX_FRAME_SURFACE_BYTES,
        SCHEMA as FRAMEBUFFER_STREAM_SCHEMA, read_frame,
    };
    use serde_json::{Value, json};
    use std::collections::{HashMap, VecDeque};
    use std::ffi::CString;
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::mem;
    use std::net::{Shutdown, TcpListener, TcpStream, UdpSocket};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    const IFACE: &str = "eth0";
    const AGENT_PORT: u16 = mister_magik_agent_protocol::PORT;
    const FRAMEBUFFER_PRODUCER_PORT: u16 = 7499;
    const TOKEN_PATH: &str = "/media/fat/mister-magik-dev/agent.token";
    const MAGIK_UIO_GET_FBUF_LATCH: u16 = mister_magik_latch_contract::GET_FBUF_LATCH;
    const MAGIK_UIO_GET_FBUF_LATCH_CAPS: u16 = mister_magik_latch_contract::GET_FBUF_LATCH_CAPS;
    const MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY: u16 =
        mister_magik_latch_contract::GET_FBUF_PRESENTATION_TELEMETRY;
    const MAGIK_FBUF_STATUS_MAGIC: u16 = mister_magik_latch_contract::STATUS_MAGIC;
    const MAGIK_FBUF_CAPS_MAGIC: u16 = mister_magik_latch_contract::CAPS_MAGIC;
    const MAGIK_FBUF_PRESENTATION_TELEMETRY_MAGIC: u16 =
        mister_magik_latch_contract::PRESENTATION_TELEMETRY_MAGIC;
    const FPGA_MGR_BASE: i64 = 0xFF70_6000;
    const FPGA_MGR_LEN: usize = 0x1000;
    const FPGA_GPO_OFF: usize = 0x10;
    const FPGA_GPI_OFF: usize = 0x14;
    const FPGA_STROBE: u32 = 1 << 17;
    const FPGA_ACK: u32 = FPGA_STROBE;
    const FPGA_IO_EN: u32 = 1 << 20;
    const FPGA_BIT31: u32 = 0x8000_0000;
    const FPGA_SPIN_LIMIT: u32 = 2_000_000;
    const CONTROL_AUTH_DISABLED: bool = false;
    const MAX_ACTIVE_CONTROL_CLIENTS: usize = 16;
    const UI_TEST_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
    pub(super) const MAX_CONTROL_REQUEST_BYTES: usize = 64 * 1024;
    const CONTROL_REQUEST_DEADLINE: Duration = Duration::from_secs(10);
    const LOG: &str = "/tmp/mister-magik-agent.log";
    const PLOG: &str = "/media/fat/mister-magik-dev/bootlogs/agent.log";
    const FRAME_ANALYTICS_LEASE_PATH: &str = "/tmp/mister-magik/realtime-frame-analytics";
    static FRAME_ANALYTICS_LEASE_GENERATION: AtomicU64 = AtomicU64::new(0);
    static FRAMEBUFFER_STREAM_ACTIVE: AtomicBool = AtomicBool::new(false);
    static ACTIVE_CONTROL_CLIENTS: ActiveControlClients =
        ActiveControlClients::new(MAX_ACTIVE_CONTROL_CLIENTS);
    static MAGIK_OPERATION_RESULTS: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new();
    const BOOTLOG_DIR: &str = "/media/fat/mister-magik-dev/bootlogs";
    const SEQ: &str = "/media/fat/mister-magik-dev/bootlogs/agent.seq";
    const CRASH_DIR: &str = "/media/fat/mister-magik-dev/crashes";
    const LATEST_CRASH_REPORT: &str = "/media/fat/mister-magik-dev/crashes/latest.json";
    const CATALOG_FAILURE_DIRS: [&str; 2] = [
        "/media/fat/mister-magik/diagnostics/catalog",
        "/media/fat/mister-magik-dev/diagnostics/catalog",
    ];

    pub(super) fn ui_test_startup_timeout(timeout_ms: u64) -> Duration {
        UI_TEST_STARTUP_TIMEOUT.min(Duration::from_millis(timeout_ms))
    }
    const CATALOG_PROGRESS_PATHS: [&str; 2] = [
        "/media/fat/mister-magik/diagnostics/catalog/progress-latest.json",
        "/media/fat/mister-magik-dev/diagnostics/catalog/progress-latest.json",
    ];
    const LATCH_IDENTITY_PATHS: [&str; 2] = [
        "/media/fat/mister-magik/diagnostics/latch/current-identity.json",
        "/media/fat/mister-magik-dev/diagnostics/latch/current-identity.json",
    ];
    const LOG_RING_CAPACITY: usize = 512;
    const TIMELINE_CAPACITY: usize = 128;

    type SharedLogRing = Arc<Mutex<LogRing>>;
    type SharedTimeline = Arc<Mutex<Timeline>>;

    static LOG_RING: OnceLock<SharedLogRing> = OnceLock::new();
    static TIMELINE: OnceLock<SharedTimeline> = OnceLock::new();

    pub(super) struct ActiveControlClients {
        active: AtomicUsize,
        limit: usize,
    }

    impl ActiveControlClients {
        pub(super) const fn new(limit: usize) -> Self {
            Self {
                active: AtomicUsize::new(0),
                limit,
            }
        }

        pub(super) fn claim(&self) -> Option<ActiveControlClient<'_>> {
            let mut active = self.active.load(Ordering::SeqCst);
            loop {
                if active >= self.limit {
                    return None;
                }
                match self.active.compare_exchange_weak(
                    active,
                    active + 1,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => return Some(ActiveControlClient { clients: self }),
                    Err(current) => active = current,
                }
            }
        }
    }

    pub(super) struct ActiveControlClient<'a> {
        clients: &'a ActiveControlClients,
    }

    impl Drop for ActiveControlClient<'_> {
        fn drop(&mut self) {
            self.clients.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
        match args.first().map(String::as_str).unwrap_or("net-boot") {
            "net-boot" => net_boot(),
            "arp" => {
                Err("ARP injection was retired; the agent no longer configures networking".into())
            }
            "-h" | "--help" => {
                eprintln!("usage: mister-magik-agent [net-boot|arp]");
                Ok(())
            }
            other => Err(format!("unknown command: {other}").into()),
        }
    }

    fn net_boot() -> Result<(), Box<dyn std::error::Error>> {
        let ring = fresh_log_ring();
        let _ = LOG_RING.set(Arc::clone(&ring));
        let timeline = fresh_timeline();
        let _ = TIMELINE.set(timeline);
        let mut log = Logger::create(LOG, ring)?;
        let boot_id = next_boot_id();
        timeline_record_once(
            "agent_start",
            format!("boot={boot_id} pid={}", std::process::id()),
        );
        log.line(format!(
            "worker_start boot={boot_id} pid={}",
            std::process::id()
        ));
        start_control_server(boot_id);

        for _ in 0..80 {
            let carrier = read_trimmed("/sys/class/net/eth0/carrier").unwrap_or_else(|| "?".into());
            let operstate =
                read_trimmed("/sys/class/net/eth0/operstate").unwrap_or_else(|| "?".into());
            log.line(format!("observed carrier={carrier} operstate={operstate}"));
            if carrier == "1" {
                timeline_record_once("carrier_up", format!("operstate={operstate}"));
                log.line(format!("carrier_ready boot={boot_id}"));
                for _ in 0..40 {
                    snapshot(boot_id, &mut log);
                    thread::sleep(Duration::from_secs(1));
                }
                persist_log(boot_id, &mut log);
                park_forever();
            }
            thread::sleep(Duration::from_millis(250));
        }

        log.line("gave_up".to_string());
        persist_log(boot_id, &mut log);
        park_forever();
    }

    fn park_forever() -> ! {
        loop {
            thread::sleep(Duration::from_secs(3600));
        }
    }

    struct Logger {
        file: File,
        ring: SharedLogRing,
    }

    impl Logger {
        fn create(path: &str, ring: SharedLogRing) -> io::Result<Self> {
            Ok(Self {
                file: File::create(path)?,
                ring,
            })
        }

        fn line(&mut self, msg: String) {
            let line = format!("{} agent {msg}", stamp());
            record_log_line(&self.ring, &line);
            let _ = writeln!(self.file, "{line}");
            let _ = self.file.flush();
        }

        fn ring_text(&self) -> String {
            ring_lines(&self.ring).join("\n")
        }
    }

    struct LogRing {
        lines: VecDeque<String>,
        dropped: u64,
    }

    impl LogRing {
        fn new() -> Self {
            Self {
                lines: VecDeque::with_capacity(LOG_RING_CAPACITY),
                dropped: 0,
            }
        }

        fn push(&mut self, line: String) {
            if self.lines.len() == LOG_RING_CAPACITY {
                self.lines.pop_front();
                self.dropped += 1;
            }
            self.lines.push_back(line);
        }
    }

    fn fresh_log_ring() -> SharedLogRing {
        Arc::new(Mutex::new(LogRing::new()))
    }

    fn record_log_line(ring: &SharedLogRing, line: &str) {
        if let Ok(mut ring) = ring.lock() {
            ring.push(line.to_string());
        }
    }

    fn ring_lines(ring: &SharedLogRing) -> Vec<String> {
        ring.lock()
            .map(|ring| ring.lines.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn log_ring_json() -> Value {
        match LOG_RING.get().and_then(|ring| ring.lock().ok()) {
            Some(ring) => json!({
                "capacity": LOG_RING_CAPACITY,
                "dropped": ring.dropped,
                "count": ring.lines.len(),
                "lines": ring.lines.iter().cloned().collect::<Vec<_>>(),
            }),
            None => json!({
                "capacity": LOG_RING_CAPACITY,
                "dropped": 0,
                "count": 0,
                "lines": [],
            }),
        }
    }

    struct Timeline {
        events: Vec<TimelineEvent>,
        dropped: u64,
    }

    struct TimelineEvent {
        name: String,
        uptime_ms: u64,
        detail: String,
    }

    impl Timeline {
        fn new() -> Self {
            Self {
                events: Vec::with_capacity(TIMELINE_CAPACITY),
                dropped: 0,
            }
        }

        fn record_once(&mut self, name: &str, detail: String) {
            if self.events.iter().any(|event| event.name == name) {
                return;
            }
            if self.events.len() == TIMELINE_CAPACITY {
                self.events.remove(0);
                self.dropped += 1;
            }
            self.events.push(TimelineEvent {
                name: name.to_string(),
                uptime_ms: uptime_ms_now(),
                detail,
            });
        }
    }

    fn fresh_timeline() -> SharedTimeline {
        Arc::new(Mutex::new(Timeline::new()))
    }

    fn timeline_record_once(name: &str, detail: String) {
        if let Some(timeline) = TIMELINE.get()
            && let Ok(mut timeline) = timeline.lock()
        {
            timeline.record_once(name, detail);
        }
    }

    fn timeline_json(boot_id: u64, started: Instant) -> Value {
        match TIMELINE.get().and_then(|timeline| timeline.lock().ok()) {
            Some(timeline) => json!({
                "boot_id": boot_id,
                "agent_uptime_ms": started.elapsed().as_millis() as u64,
                "capacity": TIMELINE_CAPACITY,
                "dropped": timeline.dropped,
                "count": timeline.events.len(),
                "events": timeline.events.iter().map(|event| {
                    json!({
                        "event": event.name,
                        "uptime_ms": event.uptime_ms,
                        "detail": event.detail,
                    })
                }).collect::<Vec<_>>(),
            }),
            None => json!({
                "boot_id": boot_id,
                "agent_uptime_ms": started.elapsed().as_millis() as u64,
                "capacity": TIMELINE_CAPACITY,
                "dropped": 0,
                "count": 0,
                "events": [],
            }),
        }
    }

    fn start_control_server(boot_id: u64) {
        let token = if CONTROL_AUTH_DISABLED {
            append_log_line(
                "control_auth_disabled accepting unauthenticated TCP commands".to_string(),
            );
            String::new()
        } else {
            let token = match fs::read_to_string(TOKEN_PATH) {
                Ok(token) => token.trim().to_string(),
                Err(err) => {
                    append_log_line(format!("control_token_missing path={TOKEN_PATH} err={err}"));
                    return;
                }
            };
            if token.is_empty() {
                append_log_line(format!("control_token_empty path={TOKEN_PATH}"));
                return;
            }
            token
        };

        thread::spawn(move || {
            let started = Instant::now();
            let token = Arc::new(token);
            let listener = match TcpListener::bind(("0.0.0.0", AGENT_PORT)) {
                Ok(listener) => listener,
                Err(err) => {
                    append_log_line(format!("control_bind_error port={AGENT_PORT} err={err}"));
                    return;
                }
            };
            append_log_line(format!("control_listen port={AGENT_PORT} boot={boot_id}"));
            timeline_record_once("control_listen", format!("port={AGENT_PORT}"));

            for stream in listener.incoming() {
                match stream {
                    Ok(mut stream) => {
                        let Some(client_guard) = ACTIVE_CONTROL_CLIENTS.claim() else {
                            let _ = writeln!(
                                stream,
                                "{}",
                                busy_failure_response(None, "agent is busy")
                            );
                            continue;
                        };
                        let token = Arc::clone(&token);
                        thread::spawn(move || {
                            let _client_guard = client_guard;
                            handle_control_client(stream, token, boot_id, started)
                        });
                    }
                    Err(err) => append_log_line(format!("control_accept_error err={err}")),
                }
            }
        });
    }

    fn handle_control_client(
        mut stream: TcpStream,
        token: Arc<String>,
        boot_id: u64,
        started: Instant,
    ) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
        let peer = stream
            .peer_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|_| "?".to_string());
        timeline_record_once("first_client_connect", format!("peer={peer}"));
        append_log_line(format!("control_client peer={peer}"));

        let mut reader = match stream.try_clone() {
            Ok(cloned) => BufReader::new(cloned),
            Err(err) => {
                let response = unavailable_failure_response(None, &format!("clone error: {err}"));
                let _ = writeln!(stream, "{response}");
                return;
            }
        };
        let read_result = read_control_request(&mut reader, |remaining| {
            stream.set_read_timeout(Some(remaining))
        });
        let response = match read_result {
            Ok(line) if line.is_empty() => failure_response(
                None,
                "empty request",
                mister_magik_agent_protocol::FailureCode::InvalidRequest,
                mister_magik_agent_protocol::FailurePhase::Request,
                mister_magik_agent_protocol::RetryPolicy::Never,
                false,
            ),
            Ok(line) => {
                if maybe_handle_ui_test_session_v1(&line, &token, &mut reader, &mut stream) {
                    return;
                }
                if maybe_handle_runtime_upload_v1(&line, &token, &mut reader, &mut stream) {
                    return;
                }
                if maybe_handle_framebuffer_stream_v1(&line, &token, &mut stream) {
                    return;
                }
                if maybe_handle_device_telemetry_stream_v2(
                    &line,
                    &token,
                    boot_id,
                    started,
                    &mut stream,
                ) {
                    return;
                }
                if maybe_handle_framebuffer_raw_stream(&line, &token, boot_id, started, &mut stream)
                {
                    return;
                }
                if maybe_handle_sd_preview_image_stream(&line, &token, &mut stream) {
                    return;
                }
                handle_control_line(&line, &token, boot_id, started)
            }
            Err(err) if err.kind() == io::ErrorKind::InvalidData => failure_response(
                None,
                &format!("read error: {err}"),
                mister_magik_agent_protocol::FailureCode::InvalidRequest,
                mister_magik_agent_protocol::FailurePhase::Request,
                mister_magik_agent_protocol::RetryPolicy::Never,
                false,
            ),
            Err(err) => unavailable_failure_response(None, &format!("read error: {err}")),
        };
        let _ = writeln!(stream, "{response}");
    }

    fn maybe_handle_ui_test_session_v1(
        line: &str,
        token: &str,
        reader: &mut BufReader<TcpStream>,
        stream: &mut TcpStream,
    ) -> bool {
        let is_session = serde_json::from_str::<Value>(line.trim())
            .ok()
            .and_then(|value| value.get("cmd").and_then(Value::as_str).map(str::to_owned))
            .as_deref()
            == Some(mister_magik_agent_protocol::UI_TEST_SESSION_COMMAND);
        if !is_session {
            return false;
        }
        let request = match parse_control_request(line, token, CONTROL_AUTH_DISABLED) {
            Ok(request) => request,
            Err(error) => {
                let response = if error.message == "unauthorized" {
                    authentication_failure_response(error.id)
                } else {
                    operation_failure_response(error.id, &error.message)
                };
                let _ = writeln!(stream, "{response}");
                return true;
            }
        };
        let request =
            match mister_magik_agent_protocol::UiTestSessionRequest::from_value(&request.args) {
                Ok(request) => request,
                Err(error) => {
                    let _ = writeln!(stream, "{}", operation_failure_response(request.id, &error));
                    return true;
                }
            };
        match run_ui_test_session(reader, stream, request) {
            Ok(()) => true,
            Err(error) => {
                let _ = writeln!(stream, "{}", operation_failure_response(None, &error));
                true
            }
        }
    }

    fn run_ui_test_session(
        reader: &mut BufReader<TcpStream>,
        stream: &mut TcpStream,
        request: mister_magik_agent_protocol::UiTestSessionRequest,
    ) -> Result<(), String> {
        if Path::new("/tmp/mister-magik/ui-automation-session.json").exists() {
            return Err("another launcher automation session is active".to_string());
        }
        let cached = crate::ui_test_runtime::prepare(&request.runtime)?;
        let runtime = if let Some(cached) = cached {
            cached
        } else {
            writeln!(
                stream,
                "{}",
                response(
                    None,
                    true,
                    Some(json!({
                        "schema": mister_magik_agent_protocol::UI_TEST_SESSION_SCHEMA,
                        "state": "upload_required",
                        "payload_bytes": request.runtime.payload_bytes,
                        "sha256": request.runtime.sha256,
                    })),
                    None,
                )
            )
            .map_err(|error| format!("send UI-test upload request: {error}"))?;
            stream
                .flush()
                .map_err(|error| format!("flush UI-test upload request: {error}"))?;
            crate::ui_test_runtime::receive(reader, &request.runtime)?
        };
        let old_status = read_json_value("/tmp/mister-magik/status.json");
        let main_status = current_main_ready()?;
        let generation = main_generation(&main_status)
            .ok_or_else(|| "Main status has no generation".to_string())?;
        let operation_id = format!(
            "ui-test-suspend-{}-{}",
            std::process::id(),
            monotonic_us_now()
        );
        magik_acknowledged_action(
            "suspend",
            &json!({"operation_id": operation_id, "expected_generation": generation}),
        )?;
        let mut child = None;
        let mut automation_nonce = None;
        let result = (|| {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .map_err(|error| format!("bind UI-test Slint endpoint: {error}"))?;
            listener
                .set_nonblocking(true)
                .map_err(|error| format!("configure UI-test Slint endpoint: {error}"))?;
            let port = listener
                .local_addr()
                .map_err(|error| format!("read UI-test Slint endpoint: {error}"))?
                .port();
            let mut command = Command::new(&runtime.path);
            command
                .args(["ui", "launcher", "0"])
                .env_clear()
                .env("SLINT_TEST_SERVER", format!("127.0.0.1:{port}"))
                .env("MISTER_UI_TEST_CASE", &request.case)
                .env("MISTER_UI_TEST_FIXTURE", &request.fixture)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            for (name, value) in &request.environment {
                command.env(name, value);
            }
            let spawned = command
                .spawn()
                .map_err(|error| format!("launch UI-test runtime: {error}"))?;
            child = Some(spawned);
            let child_pid = child.as_ref().map(Child::id).unwrap_or(0);
            let child_process = child
                .as_mut()
                .ok_or_else(|| "UI-test runtime process handle missing".to_string())?;
            wait_for_ui_test_status(
                &old_status,
                child_process,
                &request.runtime.source_revision,
                ui_test_startup_timeout(request.timeout_ms),
            )?;
            let status = read_json_value("/tmp/mister-magik/status.json");
            let build_version = status
                .pointer("/build/version")
                .and_then(Value::as_str)
                .ok_or_else(|| "UI-test runtime status has no build version".to_string())?;
            let source_revision = status
                .pointer("/build/source_revision")
                .and_then(Value::as_str)
                .ok_or_else(|| "UI-test runtime status has no source revision".to_string())?;
            let automation = crate::launcher_automation::begin(json!({
                "expected_build_version": build_version,
                "expected_source_revision": source_revision,
                "expected_main_generation": generation,
                "lifetime_seconds": (request.timeout_ms / 1_000).clamp(1, 120),
            }))?;
            automation_nonce = automation
                .get("nonce")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let nonce = automation_nonce
                .as_deref()
                .ok_or_else(|| "UI-test automation response has no nonce".to_string())?;
            let deadline = Instant::now() + Duration::from_millis(request.timeout_ms);
            let (app_stream, _) = loop {
                match listener.accept() {
                    Ok(pair) => break pair,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return Err(
                                "UI-test runtime did not connect to Slint endpoint".to_string()
                            );
                        }
                        if child
                            .as_mut()
                            .is_some_and(|child| child.try_wait().ok().flatten().is_some())
                        {
                            return Err(
                                "UI-test runtime exited before Slint connection".to_string()
                            );
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => return Err(format!("accept UI-test Slint connection: {error}")),
                }
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("UI-test session timed out before ready".to_string());
            }
            stream
                .set_read_timeout(Some(remaining))
                .map_err(|error| format!("set UI-test host read timeout: {error}"))?;
            stream
                .set_write_timeout(Some(remaining))
                .map_err(|error| format!("set UI-test host write timeout: {error}"))?;
            app_stream
                .set_read_timeout(Some(remaining))
                .map_err(|error| format!("set UI-test app read timeout: {error}"))?;
            app_stream
                .set_write_timeout(Some(remaining))
                .map_err(|error| format!("set UI-test app write timeout: {error}"))?;
            writeln!(
                stream,
                "{}",
                response(
                    None,
                    true,
                    Some(json!({
                        "schema": mister_magik_agent_protocol::UI_TEST_SESSION_SCHEMA,
                        "state": "ready",
                        "case": request.case,
                        "fixture": request.fixture,
                        "runtime_reused": runtime.reused,
                        "child_pid": child_pid,
                        "automation_nonce": nonce,
                    })),
                    None,
                )
            )
            .map_err(|error| format!("send UI-test ready response: {error}"))?;
            stream
                .flush()
                .map_err(|error| format!("flush UI-test ready response: {error}"))?;
            relay_ui_test_streams(
                stream
                    .try_clone()
                    .map_err(|error| format!("clone UI-test host stream: {error}"))?,
                app_stream,
            )
            .map_err(|error| format!("relay Slint UI-test protocol: {error}"))?;
            Ok(())
        })();
        if let Some(nonce) = automation_nonce.as_deref() {
            let _ =
                crate::launcher_automation::request(json!({"nonce": nonce, "kind": "release_all"}));
            let _ = crate::launcher_automation::request(json!({"nonce": nonce, "kind": "end"}));
        }
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_file("/tmp/mister-magik/ui-automation-session.json");
        let _ = fs::remove_file("/tmp/mister-magik/ui-automation.sock");
        let _ = fs::remove_file("/tmp/mister-magik/ui-automation-failure.json");
        let resume_generation = current_main_ready()
            .ok()
            .and_then(|status| main_generation(&status))
            .unwrap_or(generation);
        let resume_id = format!(
            "ui-test-resume-{}-{}",
            std::process::id(),
            monotonic_us_now()
        );
        let resume = magik_acknowledged_action(
            "resume",
            &json!({"operation_id": resume_id, "expected_generation": resume_generation}),
        );
        result.and(resume.map(|_| ()))
    }

    fn relay_ui_test_streams(left: TcpStream, right: TcpStream) -> Result<(), String> {
        let mut left_reader = left
            .try_clone()
            .map_err(|error| format!("clone UI-test left stream: {error}"))?;
        let mut right_writer = right
            .try_clone()
            .map_err(|error| format!("clone UI-test right stream: {error}"))?;
        let mut right_reader = right
            .try_clone()
            .map_err(|error| format!("clone UI-test right reader: {error}"))?;
        let mut left_writer = left
            .try_clone()
            .map_err(|error| format!("clone UI-test left writer: {error}"))?;
        let (done_sender, done_receiver) = std::sync::mpsc::channel();
        let forward_sender = done_sender.clone();
        let forward = thread::spawn(move || {
            let result = io::copy(&mut left_reader, &mut right_writer)
                .map(|_| ())
                .map_err(|error| error.to_string());
            let _ = forward_sender.send(result);
        });
        let reverse_sender = done_sender;
        let reverse = thread::spawn(move || {
            let result = io::copy(&mut right_reader, &mut left_writer)
                .map(|_| ())
                .map_err(|error| error.to_string());
            let _ = reverse_sender.send(result);
        });
        let first_result = done_receiver
            .recv()
            .map_err(|_| "UI-test relay threads stopped unexpectedly".to_string())?;
        let _ = left.shutdown(Shutdown::Both);
        let _ = right.shutdown(Shutdown::Both);
        forward
            .join()
            .map_err(|_| "UI-test forward relay thread panicked".to_string())?;
        reverse
            .join()
            .map_err(|_| "UI-test reverse relay thread panicked".to_string())?;
        first_result
    }

    fn wait_for_ui_test_status(
        old_status: &Value,
        child: &mut Child,
        source_revision: &str,
        timeout: Duration,
    ) -> Result<(), String> {
        let child_pid = child.id();
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let status = read_json_value("/tmp/mister-magik/status.json");
            if status.get("pid").and_then(Value::as_u64) == Some(u64::from(child_pid))
                && status
                    .pointer("/build/source_revision")
                    .and_then(Value::as_str)
                    == Some(source_revision)
                && status.get("pid") != old_status.get("pid")
            {
                return Ok(());
            }
            if let Some(exit_status) = child
                .try_wait()
                .map_err(|error| format!("check UI-test runtime status: {error}"))?
            {
                return Err(format!(
                    "UI-test runtime exited before publishing its build identity: {exit_status}"
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
        Err(format!(
            "UI-test runtime did not publish its build identity within {} ms",
            timeout.as_millis()
        ))
    }

    fn maybe_handle_runtime_upload_v1(
        line: &str,
        token: &str,
        reader: &mut impl Read,
        stream: &mut TcpStream,
    ) -> bool {
        if serde_json::from_str::<Value>(line.trim())
            .ok()
            .and_then(|value| value.get("cmd").and_then(Value::as_str).map(str::to_owned))
            .as_deref()
            != Some(mister_magik_agent_protocol::RUNTIME_UPLOAD_COMMAND)
        {
            return false;
        }

        let request = match parse_control_request(line, token, CONTROL_AUTH_DISABLED) {
            Ok(request) => request,
            Err(error) => {
                let response = if error.message == "unauthorized" {
                    authentication_failure_response(error.id)
                } else {
                    failure_response(
                        error.id,
                        &error.message,
                        mister_magik_agent_protocol::FailureCode::InvalidRequest,
                        mister_magik_agent_protocol::FailurePhase::Request,
                        mister_magik_agent_protocol::RetryPolicy::Never,
                        false,
                    )
                };
                let _ = writeln!(stream, "{response}");
                return true;
            }
        };
        let spec = match mister_magik_agent_protocol::RuntimeUploadSpec::from_args(&request.args) {
            Ok(spec) => spec,
            Err(error) => {
                let response = failure_response(
                    request.id,
                    &error,
                    mister_magik_agent_protocol::FailureCode::InvalidRequest,
                    mister_magik_agent_protocol::FailurePhase::Request,
                    mister_magik_agent_protocol::RetryPolicy::Never,
                    false,
                );
                let _ = writeln!(stream, "{response}");
                return true;
            }
        };

        let installed = mister_magik_platform_manifest_contract::DEVELOPMENT_PATHS;
        let paths = crate::runtime_upload::UploadPaths {
            lock: Path::new(installed.root).join("deploy.lock"),
            upload: PathBuf::from(format!("{}.upload", installed.gui)),
            part: PathBuf::from(format!("{}.upload.part", installed.gui)),
        };
        let response = match crate::runtime_upload::receive(reader, &spec, &paths) {
            Ok(result) => response(
                request.id,
                true,
                Some(json!({
                    "schema": mister_magik_agent_protocol::RUNTIME_UPLOAD_SCHEMA,
                    "payload_bytes": result.payload_bytes,
                    "sha256": result.sha256,
                    "receive_ms": result.receive_ms,
                    "bytes_per_second": result.bytes_per_second,
                })),
                None,
            ),
            Err(error) => match error.kind {
                crate::runtime_upload::UploadFailureKind::Busy => {
                    busy_failure_response(request.id, &error.message)
                }
                crate::runtime_upload::UploadFailureKind::Artifact => failure_response(
                    request.id,
                    &error.message,
                    mister_magik_agent_protocol::FailureCode::ArtifactMismatch,
                    mister_magik_agent_protocol::FailurePhase::Artifact,
                    mister_magik_agent_protocol::RetryPolicy::ReconcileThenRetry,
                    false,
                ),
                crate::runtime_upload::UploadFailureKind::Operation => failure_response(
                    request.id,
                    &error.message,
                    mister_magik_agent_protocol::FailureCode::OperationFailed,
                    mister_magik_agent_protocol::FailurePhase::Operation,
                    mister_magik_agent_protocol::RetryPolicy::ReconcileThenRetry,
                    false,
                ),
            },
        };
        let _ = writeln!(stream, "{response}");
        true
    }

    pub(super) fn read_control_request(
        reader: &mut impl BufRead,
        mut prepare_read: impl FnMut(Duration) -> io::Result<()>,
    ) -> io::Result<String> {
        let deadline = Instant::now() + CONTROL_REQUEST_DEADLINE;
        let mut bytes = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "control request header deadline exceeded",
                ));
            }
            prepare_read(remaining)?;
            let available = reader.fill_buf()?;
            if available.is_empty() {
                break;
            }
            let take = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            if bytes.len().saturating_add(take) > MAX_CONTROL_REQUEST_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "control request header exceeds 64 KiB",
                ));
            }
            bytes.extend_from_slice(&available[..take]);
            reader.consume(take);
            if bytes.last() == Some(&b'\n') {
                break;
            }
        }
        String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    struct ActiveFramebufferStream;

    impl ActiveFramebufferStream {
        fn claim() -> Option<Self> {
            FRAMEBUFFER_STREAM_ACTIVE
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .ok()
                .map(|_| Self)
        }
    }

    impl Drop for ActiveFramebufferStream {
        fn drop(&mut self) {
            FRAMEBUFFER_STREAM_ACTIVE.store(false, Ordering::SeqCst);
        }
    }

    fn maybe_handle_framebuffer_stream_v1(line: &str, token: &str, stream: &mut TcpStream) -> bool {
        let parsed: Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(_) => return false,
        };
        if parsed.get("cmd").and_then(Value::as_str) != Some("framebuffer_stream_v1") {
            return false;
        }
        let id = parsed.get("id").cloned();
        if !CONTROL_AUTH_DISABLED && parsed.get("token").and_then(Value::as_str) != Some(token) {
            append_log_line("control_auth_failed".to_string());
            let _ = writeln!(stream, "{}", authentication_failure_response(id));
            return true;
        }
        let Some(_guard) = ActiveFramebufferStream::claim() else {
            let _ = writeln!(
                stream,
                "{}",
                busy_failure_response(id, "framebuffer stream already has a desktop consumer",)
            );
            return true;
        };
        let mut producer = match TcpStream::connect(("127.0.0.1", FRAMEBUFFER_PRODUCER_PORT)) {
            Ok(producer) => producer,
            Err(err) => {
                let _ = writeln!(
                    stream,
                    "{}",
                    unavailable_failure_response(
                        id,
                        &format!("producer stream unavailable: {err}"),
                    )
                );
                return true;
            }
        };
        let _ = producer.set_nodelay(true);
        let _ = stream.set_nodelay(true);
        let producer_shutdown = producer.try_clone().ok();
        let desktop_reader = stream.try_clone().ok();
        if let (Some(producer_shutdown), Some(mut desktop_reader)) =
            (producer_shutdown, desktop_reader)
        {
            thread::spawn(move || {
                let mut byte = [0_u8; 1];
                let _ = desktop_reader.read(&mut byte);
                let _ = producer_shutdown.shutdown(Shutdown::Both);
            });
        }
        append_log_line("framebuffer_stream_v1_start".to_string());
        let result = json!({
            "schema": FRAMEBUFFER_STREAM_SCHEMA,
            "producer": "127.0.0.1",
            "producer_port": FRAMEBUFFER_PRODUCER_PORT,
            "source": "producer-pre-ownership-transfer",
            "ownership_safe": true,
            "encoding": "lz4-block-size-prepended",
            "format": "rgb565-le",
        });
        let _ = writeln!(stream, "{}", response(id, true, Some(result), None));
        let _ = stream.flush();
        match io::copy(&mut producer, stream) {
            Ok(bytes) => append_log_line(format!("framebuffer_stream_v1_end bytes={bytes}")),
            Err(err) => append_log_line(format!("framebuffer_stream_v1_error err={err}")),
        }
        true
    }

    fn maybe_handle_device_telemetry_stream_v2(
        line: &str,
        token: &str,
        boot_id: u64,
        started: Instant,
        stream: &mut TcpStream,
    ) -> bool {
        let parsed: Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(_) => return false,
        };
        if parsed.get("cmd").and_then(Value::as_str) != Some("device_telemetry_stream_v2") {
            return false;
        }
        let id = parsed.get("id").cloned();
        if !CONTROL_AUTH_DISABLED && parsed.get("token").and_then(Value::as_str) != Some(token) {
            append_log_line("control_auth_failed".to_string());
            let _ = writeln!(stream, "{}", authentication_failure_response(id));
            return true;
        }
        let analytics_mode = parsed
            .pointer("/args/analytics_mode")
            .and_then(Value::as_str)
            .map(normalize_frame_analytics_mode)
            .unwrap_or("process");
        let cadence = Duration::from_millis(
            parsed
                .pointer("/args/cadence_ms")
                .and_then(Value::as_u64)
                .unwrap_or(1_000)
                .clamp(100, 1_000),
        );
        append_log_line(format!(
            "device_telemetry_stream_v2_start analytics_mode={analytics_mode} cadence_ms={}",
            cadence.as_millis()
        ));
        let result = json!({
            "schema": "mister-magik-device-telemetry-stream-v2",
            "cadence_ms": cadence.as_millis(),
            "encoding": "jsonl",
        });
        let _ = stream.set_nodelay(true);
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        if writeln!(stream, "{}", response(id, true, Some(result), None)).is_err() {
            return true;
        }
        if stream.flush().is_err() {
            return true;
        }

        let mut state = DeviceTelemetryStreamState::default();
        let mut seq = 0_u64;
        loop {
            let sample_started = Instant::now();
            let sample_started_monotonic_us = monotonic_us_now();
            let lease_started = Instant::now();
            refresh_frame_analytics_lease(analytics_mode);
            let lease_publication_us = lease_started.elapsed().as_micros() as u64;
            let snapshot = state.snapshot(
                seq,
                boot_id,
                started,
                sample_started_monotonic_us,
                lease_publication_us,
            );
            let serialization_started = Instant::now();
            let encoded = snapshot.to_string();
            let serialization_us = serialization_started.elapsed().as_micros() as u64;
            let socket_started = Instant::now();
            if writeln!(stream, "{encoded}").is_err() || stream.flush().is_err() {
                break;
            }
            let socket_write_us = socket_started.elapsed().as_micros() as u64;
            state.complete_transport_sample(
                serialization_us,
                socket_write_us,
                encoded.len() as u64 + 1,
                sample_started.elapsed(),
                cadence,
            );
            seq = seq.saturating_add(1);
            let elapsed = sample_started.elapsed();
            if elapsed < cadence {
                thread::sleep(cadence - elapsed);
            }
        }
        clear_frame_analytics_lease();
        append_log_line("device_telemetry_stream_v2_end".to_string());
        true
    }

    fn normalize_frame_analytics_mode(mode: &str) -> &'static str {
        match mode {
            "off" => "off",
            "wall" => "wall",
            "thread" => "thread",
            _ => "process",
        }
    }

    fn refresh_frame_analytics_lease(mode: &str) {
        if mode == "off" {
            clear_frame_analytics_lease();
            return;
        }
        let generation = FRAME_ANALYTICS_LEASE_GENERATION.fetch_add(1, Ordering::Relaxed);
        refresh_frame_analytics_lease_at(Path::new(FRAME_ANALYTICS_LEASE_PATH), mode, generation);
    }

    pub(super) fn refresh_frame_analytics_lease_at(path: &Path, mode: &str, generation: u64) {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let temp = path.with_extension(format!("lease-{}-{generation}", std::process::id()));
        let published = fs::write(&temp, format!("{mode}\n"))
            .and_then(|()| fs::rename(&temp, path))
            .is_ok();
        if !published {
            let _ = fs::remove_file(temp);
        }
    }

    fn clear_frame_analytics_lease() {
        let _ = fs::remove_file(FRAME_ANALYTICS_LEASE_PATH);
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(super) struct CpuTimes {
        pub(super) user: u64,
        pub(super) nice: u64,
        pub(super) system: u64,
        pub(super) idle: u64,
        pub(super) iowait: u64,
        pub(super) irq: u64,
        pub(super) softirq: u64,
        pub(super) steal: u64,
    }

    impl CpuTimes {
        fn total(self) -> u64 {
            self.user
                .saturating_add(self.nice)
                .saturating_add(self.system)
                .saturating_add(self.idle)
                .saturating_add(self.iowait)
                .saturating_add(self.irq)
                .saturating_add(self.softirq)
                .saturating_add(self.steal)
        }

        fn idle_total(self) -> u64 {
            self.idle.saturating_add(self.iowait)
        }
    }

    #[derive(Clone, Copy, Debug, Default)]
    pub(super) struct NetSample {
        pub(super) rx_bytes: u64,
        pub(super) tx_bytes: u64,
        pub(super) at: Option<Instant>,
    }

    const SD_READ_BYTES_PER_SEC_AT_100_PCT: u64 = 50_000_000;
    const SD_WRITE_BYTES_PER_SEC_AT_100_PCT: u64 = 25_000_000;
    const DISK_SECTOR_BYTES: u64 = 512;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(super) struct DiskCounters {
        pub(super) sectors_read: u64,
        pub(super) sectors_written: u64,
    }

    #[derive(Clone, Debug, Default)]
    struct DiskSample {
        device: String,
        counters: DiskCounters,
        at: Option<Instant>,
    }

    #[derive(Default)]
    struct DeviceTelemetryStreamState {
        previous_cpu: Option<Vec<CpuTimes>>,
        previous_net: Option<NetSample>,
        previous_disk: Option<DiskSample>,
        fpga: Option<FpgaIo>,
        previous_transport: Option<TelemetryTransportEvidence>,
    }

    #[derive(Clone, Copy, Debug, Default)]
    struct TelemetryTransportEvidence {
        json_serialization_us: u64,
        socket_write_us: u64,
        bytes_serialized: u64,
        sample_deadline_overrun_us: u64,
    }

    #[derive(Clone, Copy, Debug, Default)]
    pub(super) struct ProcessTelemetryEvidence {
        pub(super) discovery_us: u64,
        pub(super) proc_parse_us: u64,
        pub(super) child_processes: u64,
        pub(super) files_read: u64,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(super) struct ProcessStatusFields {
        pub(super) rss_kb: u64,
        pub(super) threads: u64,
    }

    #[derive(Debug, Default)]
    pub(super) struct ProcessTelemetrySnapshot {
        pub(super) processes: HashMap<&'static str, Value>,
        pub(super) evidence: ProcessTelemetryEvidence,
    }

    #[cfg(test)]
    pub(super) fn aggregate_process_telemetry_evidence(
        items: &[ProcessTelemetryEvidence],
    ) -> ProcessTelemetryEvidence {
        items
            .iter()
            .fold(ProcessTelemetryEvidence::default(), |mut total, item| {
                total.discovery_us = total.discovery_us.saturating_add(item.discovery_us);
                total.proc_parse_us = total.proc_parse_us.saturating_add(item.proc_parse_us);
                total.child_processes = total.child_processes.saturating_add(item.child_processes);
                total.files_read = total.files_read.saturating_add(item.files_read);
                total
            })
    }

    pub(super) fn telemetry_deadline_overrun_us(elapsed: Duration, cadence: Duration) -> u64 {
        elapsed.saturating_sub(cadence).as_micros() as u64
    }

    impl DeviceTelemetryStreamState {
        fn snapshot(
            &mut self,
            seq: u64,
            boot_id: u64,
            started: Instant,
            sample_started_monotonic_us: u64,
            lease_publication_us: u64,
        ) -> Value {
            let cpu_started = Instant::now();
            let cpu_times = read_cpu_times().unwrap_or_default();
            let cpu = cpu_json(self.previous_cpu.as_deref(), &cpu_times);
            self.previous_cpu = Some(cpu_times);
            let cpu_read_us = cpu_started.elapsed().as_micros() as u64;

            let network_started = Instant::now();
            let net_fields = read_netdev_stats_fields(IFACE);
            let now = Instant::now();
            let network = network_json(self.previous_net, net_fields, now);
            self.previous_net = net_fields.map(|fields| NetSample {
                rx_bytes: fields[0],
                tx_bytes: fields[8],
                at: Some(now),
            });
            let network_read_us = network_started.elapsed().as_micros() as u64;

            let disk_started = Instant::now();
            let disk_device = backing_disk_for_path("/media/fat");
            let disk_counters = disk_device.as_deref().and_then(read_disk_counters);
            let disk_activity = disk_activity_json(
                self.previous_disk.as_ref(),
                disk_device.as_deref(),
                disk_counters,
                now,
            );
            self.previous_disk =
                disk_device
                    .zip(disk_counters)
                    .map(|(device, counters)| DiskSample {
                        device,
                        counters,
                        at: Some(now),
                    });
            let disk_read_us = disk_started.elapsed().as_micros() as u64;

            let process_snapshot = process_telemetry_snapshot();
            let magik = process_snapshot
                .processes
                .get("mister-magik-fb")
                .cloned()
                .unwrap_or_else(empty_process_telemetry);
            let main_dev = process_snapshot
                .processes
                .get("MiSTer_MagiKDev")
                .cloned()
                .unwrap_or_else(empty_process_telemetry);
            let main_public = process_snapshot
                .processes
                .get("MiSTer_MagiK")
                .cloned()
                .unwrap_or_else(empty_process_telemetry);
            let process_evidence = process_snapshot.evidence;
            let mut files_read = 4_u64.saturating_add(process_evidence.files_read);
            let main = if main_dev
                .get("pids")
                .and_then(Value::as_array)
                .is_some_and(|pids| !pids.is_empty())
            {
                &main_dev
            } else {
                &main_public
            };
            let magik_rss_kb = magik.get("rss_kb").and_then(Value::as_u64).unwrap_or(0);
            let main_rss_kb = main.get("rss_kb").and_then(Value::as_u64).unwrap_or(0);
            let status_started = Instant::now();
            let slint_status = read_json_value("/tmp/mister-magik/status.json");
            let ui_thread_cpu =
                launcher_ui_pid(&slint_status, &magik["pids"]).and_then(main_thread_current_cpu);
            let slint_current = status_pid_matches(&slint_status, &magik["pids"]);
            files_read = files_read.saturating_add(1 + u64::from(ui_thread_cpu.is_some()));
            let status_parsing_us = status_started.elapsed().as_micros() as u64;
            let memory_started = Instant::now();
            let memory = memory_json(magik_rss_kb, main_rss_kb);
            files_read = files_read.saturating_add(1);
            let memory_read_us = memory_started.elapsed().as_micros() as u64;
            let storage_started = Instant::now();
            let storage = storage_json("/media/fat", disk_activity);
            let storage_read_us = storage_started.elapsed().as_micros() as u64;
            let fpga_started = Instant::now();
            let presentation = self.presentation_telemetry_json();
            let fpga_telemetry_us = fpga_started.elapsed().as_micros() as u64;
            let assembly_started = Instant::now();
            let mut payload = json!({
                "schema": "mister-magik-device-telemetry-v2",
                "seq": seq,
                "agent": {
                    "boot_id": boot_id,
                    "uptime_ms": started.elapsed().as_millis() as u64,
                },
                "cpu": cpu,
                "memory": memory,
                "processes": {
                    "mister-magik-fb": magik,
                    "MiSTer_MagiKDev": main_dev,
                    "MiSTer_MagiK": main_public,
                },
                "network": network,
                "storage": storage,
                "presentation": presentation,
                "launcher": {
                    "status_current": slint_current,
                    "status_sequence": slint_status.get("status_sequence").cloned().unwrap_or(Value::Null),
                    "status_publish_mode": slint_status.get("status_publish_mode").cloned().unwrap_or(Value::Null),
                    "status_submitted_sequence": slint_status.get("status_submitted_sequence").cloned().unwrap_or(Value::Null),
                    "status_written_sequence": slint_status.get("status_written_sequence").cloned().unwrap_or(Value::Null),
                    "status_replaced_count": slint_status.get("status_replaced_count").cloned().unwrap_or(Value::Null),
                    "status_worker_write_us": slint_status.get("status_worker_write_us").cloned().unwrap_or(Value::Null),
                    "status_worker_errors": slint_status.get("status_worker_errors").cloned().unwrap_or(Value::Null),
                    "idle": slint_status.get("idle").cloned().unwrap_or(Value::Null),
                    "screen": slint_status.get("screen").cloned().unwrap_or(Value::Null),
                    "composition_state": slint_status.get("composition_state").cloned().unwrap_or(Value::Null),
                    "output_route": slint_status.get("output_route").cloned().unwrap_or(Value::Null),
                    "framebuffer_width": slint_status.get("framebuffer_width").cloned().unwrap_or(Value::Null),
                    "framebuffer_height": slint_status.get("framebuffer_height").cloned().unwrap_or(Value::Null),
                    "catalog_ready": slint_status.get("catalog_ready").cloned().unwrap_or(Value::Null),
                    "catalog_refresh_policy": slint_status.get("catalog_refresh_policy").cloned().unwrap_or(Value::Null),
                    "catalog_worker_enabled": slint_status.get("catalog_worker_enabled").cloned().unwrap_or(Value::Null),
                    "screensaver_profile_state": slint_status.get("screensaver_profile_state").cloned().unwrap_or(Value::Null),
                    "present_backend": slint_status.get("present_backend").cloned().unwrap_or(Value::Null),
                    "present_status": slint_status.get("present_status").cloned().unwrap_or(Value::Null),
                    "latch_drop_count": slint_status.get("latch_drop_count").cloned().unwrap_or(Value::Null),
                    "rolling_fps": slint_status.get("rolling_fps").cloned().unwrap_or(Value::Null),
                    "fps_estimate": slint_status.get("fps_estimate").cloned().unwrap_or(Value::Null),
                    "preview_cache_state": slint_status.get("preview_cache_state").cloned().unwrap_or(Value::Null),
                    "frame_budget": slint_status.get("frame_budget").cloned().unwrap_or(Value::Null),
                    "ui_thread_cpu": ui_thread_cpu,
                    "last_error": Value::Null,
                },
            });
            let json_assembly_us = assembly_started.elapsed().as_micros() as u64;
            let previous_transport = self.previous_transport.unwrap_or_default();
            payload["observer"] = json!({
                "schema": "mister-magik-agent-telemetry-phase-evidence-v1",
                "clock_domain": "CLOCK_MONOTONIC",
                "sample_started_monotonic_us": sample_started_monotonic_us,
                "sample_assembled_monotonic_us": monotonic_us_now(),
                "phases_us": {
                    "process_discovery": process_evidence.discovery_us,
                    "proc_parsing": process_evidence.proc_parse_us,
                    "cpu_read": cpu_read_us,
                    "network_read": network_read_us,
                    "disk_read": disk_read_us,
                    "memory_read": memory_read_us,
                    "storage_read": storage_read_us,
                    "status_parsing": status_parsing_us,
                    "lease_publication": lease_publication_us,
                    "fpga_telemetry": fpga_telemetry_us,
                    "json_assembly": json_assembly_us,
                    "previous_json_serialization": previous_transport.json_serialization_us,
                    "previous_socket_write": previous_transport.socket_write_us,
                },
                "counts": {
                    "child_processes": process_evidence.child_processes,
                    "files_read": files_read,
                    "previous_bytes_serialized": previous_transport.bytes_serialized,
                },
                "previous_sample_deadline_overrun_us": previous_transport.sample_deadline_overrun_us,
                "transport_is_previous_sample": self.previous_transport.is_some(),
            });
            payload
        }

        fn complete_transport_sample(
            &mut self,
            json_serialization_us: u64,
            socket_write_us: u64,
            bytes_serialized: u64,
            elapsed: Duration,
            cadence: Duration,
        ) {
            self.previous_transport = Some(TelemetryTransportEvidence {
                json_serialization_us,
                socket_write_us,
                bytes_serialized,
                sample_deadline_overrun_us: telemetry_deadline_overrun_us(elapsed, cadence),
            });
        }

        fn presentation_telemetry_json(&mut self) -> Value {
            if self.fpga.is_none() {
                match FpgaIo::open() {
                    Ok(fpga) => self.fpga = Some(fpga),
                    Err(error) => return unavailable_presentation_telemetry(error.to_string()),
                }
            }
            let fpga = self.fpga.as_mut().expect("FPGA opened above");
            let result = fpga
                .lock_uio_transaction()
                .and_then(|_guard| fpga.read_presentation_telemetry());
            match result {
                Ok(telemetry) => presentation_telemetry_json(telemetry, monotonic_us_now()),
                Err(error) => {
                    self.fpga = None;
                    unavailable_presentation_telemetry(error.to_string())
                }
            }
        }
    }

    pub(super) fn presentation_telemetry_json(
        telemetry: mister_magik_latch_contract::PresentationTelemetry,
        captured_monotonic_us: u64,
    ) -> Value {
        json!({
            "schema": "mister-magik-presentation-telemetry-snapshot-v1",
            "source": "fpga-owned-vblank-telemetry",
            "available": true,
            "captured_monotonic_us": captured_monotonic_us,
            "owned_vblank_count": telemetry.owned_vblank_count,
            "presented_vblank_count": telemetry.presented_vblank_count,
            "repeated_vblank_count": telemetry.repeated_vblank_count,
            "ownership_loss_count": telemetry.ownership_loss_count,
            "active_sequence": telemetry.active_sequence,
            "flags": telemetry.flags,
            "magik_ownership": telemetry.magik_ownership(),
            "pending": telemetry.pending(),
            "lifetime_invariant_valid": telemetry.lifetime_invariant_valid(),
            "error": Value::Null,
        })
    }

    pub(super) fn unavailable_presentation_telemetry(error: String) -> Value {
        json!({
            "schema": "mister-magik-presentation-telemetry-snapshot-v1",
            "source": "fpga-owned-vblank-telemetry",
            "available": false,
            "captured_monotonic_us": monotonic_us_now(),
            "error": error,
        })
    }

    pub(super) fn parse_cpu_times_text(text: &str) -> Vec<CpuTimes> {
        text.lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let label = fields.next()?;
                if label != "cpu"
                    && !label
                        .strip_prefix("cpu")?
                        .chars()
                        .all(|c| c.is_ascii_digit())
                {
                    return None;
                }
                let nums = fields
                    .take(8)
                    .map(|field| field.parse::<u64>().unwrap_or(0))
                    .collect::<Vec<_>>();
                Some(CpuTimes {
                    user: *nums.first().unwrap_or(&0),
                    nice: *nums.get(1).unwrap_or(&0),
                    system: *nums.get(2).unwrap_or(&0),
                    idle: *nums.get(3).unwrap_or(&0),
                    iowait: *nums.get(4).unwrap_or(&0),
                    irq: *nums.get(5).unwrap_or(&0),
                    softirq: *nums.get(6).unwrap_or(&0),
                    steal: *nums.get(7).unwrap_or(&0),
                })
            })
            .collect()
    }

    fn read_cpu_times() -> Option<Vec<CpuTimes>> {
        fs::read_to_string("/proc/stat")
            .ok()
            .map(|text| parse_cpu_times_text(&text))
    }

    pub(super) fn cpu_busy_percent(previous: CpuTimes, current: CpuTimes) -> f64 {
        let total_delta = current.total().saturating_sub(previous.total());
        if total_delta == 0 {
            return 0.0;
        }
        let idle_delta = current.idle_total().saturating_sub(previous.idle_total());
        let busy = total_delta.saturating_sub(idle_delta);
        ((busy as f64 * 1000.0 / total_delta as f64).round()) / 10.0
    }

    fn cpu_json(previous: Option<&[CpuTimes]>, current: &[CpuTimes]) -> Value {
        let combined = match (previous.and_then(|items| items.first()), current.first()) {
            (Some(prev), Some(now)) => cpu_busy_percent(*prev, *now),
            _ => 0.0,
        };
        let cores = current
            .iter()
            .enumerate()
            .skip(1)
            .map(|(index, now)| {
                let busy_pct = previous
                    .and_then(|items| items.get(index))
                    .map(|prev| cpu_busy_percent(*prev, *now))
                    .unwrap_or(0.0);
                json!({"id": index - 1, "busy_pct": busy_pct})
            })
            .collect::<Vec<_>>();
        json!({
            "combined_busy_pct": combined,
            "cores": cores,
        })
    }

    pub(super) fn memory_split_json(
        mem_total_kb: u64,
        mem_available_kb: u64,
        magik_rss_kb: u64,
        main_rss_kb: u64,
    ) -> Value {
        let available_kb = mem_available_kb.min(mem_total_kb);
        let magik_kb = magik_rss_kb.min(mem_total_kb);
        let used_without_available = mem_total_kb.saturating_sub(available_kb);
        let other_used_kb = used_without_available.saturating_sub(magik_kb);
        json!({
            "total_kb": mem_total_kb,
            "available_kb": available_kb,
            "magik_kb": magik_kb,
            "main_kb": main_rss_kb,
            "other_used_kb": other_used_kb,
            "available_pct": percent_of(available_kb, mem_total_kb),
            "magik_pct": percent_of(magik_kb, mem_total_kb),
            "other_used_pct": percent_of(other_used_kb, mem_total_kb),
        })
    }

    fn memory_json(magik_rss_kb: u64, main_rss_kb: u64) -> Value {
        let meminfo = read_meminfo();
        let total = meminfo_value(&meminfo, "MemTotal").unwrap_or(0);
        let available = meminfo_value(&meminfo, "MemAvailable").unwrap_or_else(|| {
            meminfo_value(&meminfo, "MemFree").unwrap_or(0)
                + meminfo_value(&meminfo, "Buffers").unwrap_or(0)
                + meminfo_value(&meminfo, "Cached").unwrap_or(0)
        });
        memory_split_json(total, available, magik_rss_kb, main_rss_kb)
    }

    fn read_meminfo() -> Vec<(String, u64)> {
        fs::read_to_string("/proc/meminfo")
            .ok()
            .map(|text| {
                text.lines()
                    .filter_map(|line| {
                        let (key, rest) = line.split_once(':')?;
                        let value = rest.split_whitespace().next()?.parse::<u64>().ok()?;
                        Some((key.to_string(), value))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn meminfo_value(items: &[(String, u64)], key: &str) -> Option<u64> {
        items
            .iter()
            .find_map(|(item_key, value)| (item_key == key).then_some(*value))
    }

    fn process_telemetry_snapshot() -> ProcessTelemetrySnapshot {
        process_telemetry_snapshot_at(Path::new("/proc"))
    }

    pub(super) fn process_telemetry_snapshot_at(proc_root: &Path) -> ProcessTelemetrySnapshot {
        const TARGETS: [&str; 3] = ["mister-magik-fb", "MiSTer_MagiKDev", "MiSTer_MagiK"];
        let discovery_started = Instant::now();
        let mut matched = HashMap::<&'static str, Vec<u64>>::from([
            (TARGETS[0], Vec::new()),
            (TARGETS[1], Vec::new()),
            (TARGETS[2], Vec::new()),
        ]);
        let mut files_read = 0u64;
        if let Ok(entries) = fs::read_dir(proc_root) {
            for entry in entries.flatten() {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u64>().ok())
                else {
                    continue;
                };
                let Ok(comm) = fs::read_to_string(entry.path().join("comm")) else {
                    continue;
                };
                files_read = files_read.saturating_add(1);
                let name = comm.trim();
                if let Some(pids) = matched.get_mut(name) {
                    pids.push(pid);
                }
            }
        }
        for pids in matched.values_mut() {
            pids.sort_unstable();
        }
        let discovery_us = discovery_started.elapsed().as_micros() as u64;

        let parse_started = Instant::now();
        let mut processes = HashMap::new();
        for target in TARGETS {
            let pids = &matched[target];
            let mut rss_kb = 0u64;
            let mut threads = 0u64;
            for pid in pids {
                let Ok(status) = fs::read_to_string(proc_root.join(pid.to_string()).join("status"))
                else {
                    continue;
                };
                files_read = files_read.saturating_add(1);
                let fields = parse_process_status(&status);
                rss_kb = rss_kb.saturating_add(fields.rss_kb);
                threads = threads.saturating_add(fields.threads);
            }
            processes.insert(
                target,
                json!({
                    "pids": pids,
                    "rss_kb": rss_kb,
                    "threads": threads,
                }),
            );
        }
        let proc_parse_us = parse_started.elapsed().as_micros() as u64;
        ProcessTelemetrySnapshot {
            processes,
            evidence: ProcessTelemetryEvidence {
                discovery_us,
                proc_parse_us,
                child_processes: 0,
                files_read,
            },
        }
    }

    fn empty_process_telemetry() -> Value {
        json!({"pids": [], "rss_kb": 0, "threads": 0})
    }

    pub(super) fn parse_process_status(text: &str) -> ProcessStatusFields {
        let mut fields = ProcessStatusFields::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            match key {
                "VmRSS" => fields.rss_kb = value,
                "Threads" => fields.threads = value,
                _ => {}
            }
        }
        fields
    }

    fn main_thread_current_cpu(pid: u64) -> Option<u64> {
        let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_proc_stat_processor(&text)
    }

    pub(super) fn parse_proc_stat_processor(text: &str) -> Option<u64> {
        // `comm` is parenthesized and may contain spaces, so count fields only
        // after its final closing delimiter. Processor is Linux stat field 39.
        text.rsplit_once(") ")?
            .1
            .split_whitespace()
            .nth(36)
            .and_then(|value| value.parse::<u64>().ok())
    }

    pub(super) fn network_rate_json(previous: Option<NetSample>, current: NetSample) -> Value {
        let elapsed = previous
            .and_then(|previous| {
                Some(
                    current
                        .at?
                        .saturating_duration_since(previous.at?)
                        .as_secs_f64(),
                )
            })
            .unwrap_or(0.0);
        let (rx_bps, tx_bps) = if elapsed > 0.0 {
            let previous = previous.unwrap_or_default();
            (
                ((current.rx_bytes.saturating_sub(previous.rx_bytes) as f64) / elapsed).round()
                    as u64,
                ((current.tx_bytes.saturating_sub(previous.tx_bytes) as f64) / elapsed).round()
                    as u64,
            )
        } else {
            (0, 0)
        };
        json!({
            "rx_bytes": current.rx_bytes,
            "tx_bytes": current.tx_bytes,
            "rx_bytes_per_sec": rx_bps,
            "tx_bytes_per_sec": tx_bps,
        })
    }

    fn network_json(previous: Option<NetSample>, fields: Option<[u64; 16]>, now: Instant) -> Value {
        match fields {
            Some(fields) => network_rate_json(
                previous,
                NetSample {
                    rx_bytes: fields[0],
                    tx_bytes: fields[8],
                    at: Some(now),
                },
            ),
            None => Value::Null,
        }
    }

    pub(super) fn parse_backing_disk(mounts: &str, diskstats: &str, path: &str) -> Option<String> {
        let source = mounts.lines().find_map(|line| {
            let mut fields = line.split_whitespace();
            let source = fields.next()?;
            let mountpoint = fields.next()?;
            (mountpoint == path).then_some(source)
        });
        if let Some(source) = source {
            let base = source.rsplit('/').next().unwrap_or(source);
            if let Some(index) = base.rfind('p')
                && base.starts_with("mmcblk")
                && base[index + 1..].chars().all(|c| c.is_ascii_digit())
            {
                return Some(base[..index].to_string());
            }
            if base.starts_with("sd") {
                return Some(
                    base.trim_end_matches(|c: char| c.is_ascii_digit())
                        .to_string(),
                );
            }
        }
        let candidates = diskstats
            .lines()
            .filter_map(|line| {
                let device = line.split_whitespace().nth(2)?;
                (device.starts_with("mmcblk")
                    && !device.contains('p')
                    && device[6..].chars().all(|c| c.is_ascii_digit()))
                .then(|| device.to_string())
            })
            .collect::<Vec<_>>();
        (candidates.len() == 1).then(|| candidates[0].clone())
    }

    fn backing_disk_for_path(path: &str) -> Option<String> {
        let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
        let diskstats = fs::read_to_string("/proc/diskstats").unwrap_or_default();
        parse_backing_disk(&mounts, &diskstats, path)
    }

    pub(super) fn parse_disk_counters(diskstats: &str, device: &str) -> Option<DiskCounters> {
        diskstats.lines().find_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.get(2).copied() != Some(device) {
                return None;
            }
            Some(DiskCounters {
                sectors_read: fields.get(5)?.parse().ok()?,
                sectors_written: fields.get(9)?.parse().ok()?,
            })
        })
    }

    fn read_disk_counters(device: &str) -> Option<DiskCounters> {
        parse_disk_counters(&fs::read_to_string("/proc/diskstats").ok()?, device)
    }

    pub(super) fn disk_rate_bytes_per_sec(
        previous: DiskCounters,
        current: DiskCounters,
        elapsed: Duration,
    ) -> Option<(u64, u64)> {
        if elapsed.is_zero()
            || current.sectors_read < previous.sectors_read
            || current.sectors_written < previous.sectors_written
        {
            return None;
        }
        let seconds = elapsed.as_secs_f64();
        Some((
            ((current.sectors_read - previous.sectors_read) as f64 * DISK_SECTOR_BYTES as f64
                / seconds)
                .round() as u64,
            ((current.sectors_written - previous.sectors_written) as f64 * DISK_SECTOR_BYTES as f64
                / seconds)
                .round() as u64,
        ))
    }

    pub(super) fn throughput_percent(bytes_per_sec: u64, ceiling: u64) -> f64 {
        if ceiling == 0 {
            return 0.0;
        }
        (bytes_per_sec as f64 * 100.0 / ceiling as f64).clamp(0.0, 100.0)
    }

    fn disk_activity_json(
        previous: Option<&DiskSample>,
        device: Option<&str>,
        current: Option<DiskCounters>,
        now: Instant,
    ) -> Value {
        let rates = previous
            .zip(device)
            .zip(current)
            .filter(|((previous, device), _)| previous.device == *device)
            .and_then(|((previous, _), current)| {
                disk_rate_bytes_per_sec(
                    previous.counters,
                    current,
                    now.saturating_duration_since(previous.at?),
                )
            });
        let valid = rates.is_some();
        let rates = rates.unwrap_or((0, 0));
        json!({
            "device": device.unwrap_or(""),
            "activity_valid": valid,
            "read_bytes_per_sec": rates.0,
            "write_bytes_per_sec": rates.1,
            "read_pct": throughput_percent(rates.0, SD_READ_BYTES_PER_SEC_AT_100_PCT),
            "write_pct": throughput_percent(rates.1, SD_WRITE_BYTES_PER_SEC_AT_100_PCT),
        })
    }

    fn storage_json(path: &str, activity: Value) -> Value {
        let Ok(c_path) = CString::new(path) else {
            return Value::Null;
        };
        let mut stats = mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: statvfs writes a valid statvfs struct when it returns 0; c_path is NUL-terminated.
        let rc = unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) };
        if rc != 0 {
            return Value::Null;
        }
        // SAFETY: statvfs returned success, so stats is initialized.
        let stats = unsafe { stats.assume_init() };
        let block_size = statvfs_value_to_u64(stats.f_frsize);
        let total_bytes = statvfs_value_to_u64(stats.f_blocks).saturating_mul(block_size);
        let available_bytes = statvfs_value_to_u64(stats.f_bavail).saturating_mul(block_size);
        let mut storage = json!({
            "path": path,
            "total_bytes": total_bytes,
            "available_bytes": available_bytes,
            "used_bytes": total_bytes.saturating_sub(available_bytes),
            "available_pct": percent_of(available_bytes, total_bytes),
        });
        if let (Some(storage), Some(activity)) = (storage.as_object_mut(), activity.as_object()) {
            storage.extend(activity.clone());
        }
        storage
    }

    #[cfg(target_pointer_width = "64")]
    fn statvfs_value_to_u64(value: u64) -> u64 {
        value
    }

    #[cfg(target_pointer_width = "32")]
    fn statvfs_value_to_u64<T: Into<u64>>(value: T) -> u64 {
        value.into()
    }

    fn percent_of(value: u64, total: u64) -> f64 {
        if total == 0 {
            0.0
        } else {
            ((value as f64 * 1000.0 / total as f64).round()) / 10.0
        }
    }

    fn maybe_handle_framebuffer_raw_stream(
        line: &str,
        token: &str,
        boot_id: u64,
        started: Instant,
        stream: &mut TcpStream,
    ) -> bool {
        let request_received = Instant::now();
        let request_received_monotonic_us = monotonic_us_now();
        let parsed: Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(_) => return false,
        };
        let cmd = parsed.get("cmd").and_then(Value::as_str);
        let lz4 = match cmd {
            Some("framebuffer_capture_raw_stream") => false,
            Some("framebuffer_capture_lz4_stream") => true,
            _ => return false,
        };
        let id = parsed.get("id").cloned();
        if !CONTROL_AUTH_DISABLED && parsed.get("token").and_then(Value::as_str) != Some(token) {
            append_log_line("control_auth_failed".to_string());
            let _ = writeln!(stream, "{}", authentication_failure_response(id));
            return true;
        }
        timeline_record_once("first_command", format!("cmd={}", cmd.unwrap_or("")));
        match framebuffer_capture_raw(
            request_received,
            request_received_monotonic_us,
            started,
            boot_id,
            lz4,
        ) {
            Ok(capture) => {
                let response = response(id, true, Some(capture.result), None);
                let _ = writeln!(stream, "{response}");
                let _ = stream.write_all(&capture.payload);
            }
            Err(err) => {
                let _ = writeln!(stream, "{}", operation_failure_response(id, &err));
            }
        }
        true
    }

    fn maybe_handle_sd_preview_image_stream(
        line: &str,
        token: &str,
        stream: &mut TcpStream,
    ) -> bool {
        let parsed: Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(_) => return false,
        };
        if parsed.get("cmd").and_then(Value::as_str) != Some("sd_read_preview_image_v1") {
            return false;
        }
        let id = parsed.get("id").cloned();
        if !CONTROL_AUTH_DISABLED && parsed.get("token").and_then(Value::as_str) != Some(token) {
            append_log_line("control_auth_failed".to_string());
            let _ = writeln!(stream, "{}", authentication_failure_response(id));
            return true;
        }
        let args = parsed.get("args").cloned().unwrap_or_else(|| json!({}));
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(crate::sd_browse::ROOT_PATH);
        timeline_record_once("first_command", "cmd=sd_read_preview_image_v1".to_string());
        match crate::sd_browse::preview_image_at_root(Path::new(crate::sd_browse::SD_ROOT), path) {
            Ok(preview) => {
                let response = response(id, true, Some(preview.result), None);
                let _ = writeln!(stream, "{response}");
                let _ = stream.write_all(&preview.payload);
            }
            Err(err) => {
                let _ = writeln!(stream, "{}", operation_failure_response(id, &err));
            }
        }
        true
    }

    fn handle_control_line(line: &str, token: &str, boot_id: u64, started: Instant) -> String {
        let request_received = Instant::now();
        let request_received_monotonic_us = monotonic_us_now();
        let request = match parse_control_request(line, token, CONTROL_AUTH_DISABLED) {
            Ok(request) => request,
            Err(error) => {
                if error.message == "unauthorized" {
                    append_log_line("control_auth_failed".to_string());
                }
                if error.message == "unauthorized" {
                    return authentication_failure_response(error.id);
                }
                return failure_response(
                    error.id,
                    &error.message,
                    mister_magik_agent_protocol::FailureCode::InvalidRequest,
                    mister_magik_agent_protocol::FailurePhase::Request,
                    mister_magik_agent_protocol::RetryPolicy::Never,
                    false,
                );
            }
        };
        let ControlRequest { id, cmd, args } = request;
        timeline_record_once("first_command", format!("cmd={cmd}"));

        match cmd.as_str() {
            "ping" => response(
                id,
                true,
                Some(json!({
                    "pong": true,
                    "agent_version": mister_magik_agent_protocol::AGENT_VERSION,
                    "protocol_version": mister_magik_agent_protocol::PROTOCOL_VERSION,
                    "capabilities": [
                        mister_magik_agent_protocol::FRAMEBUFFER_CAPTURE_CAPABILITY,
                        mister_magik_agent_protocol::DEVICE_TELEMETRY_CAPABILITY,
                        mister_magik_agent_protocol::LAUNCHER_AUTOMATION_CAPABILITY,
                        mister_magik_agent_protocol::UI_TEST_CAPABILITY,
                        mister_magik_agent_protocol::UI_TEST_SESSION_CAPABILITY,
                        mister_magik_agent_protocol::ALPHA_CANDIDATE_INSTALL_CAPABILITY,
                        mister_magik_agent_protocol::RUNTIME_UPLOAD_CAPABILITY,
                    ],
                })),
                None,
            ),
            "status" => response(id, true, Some(status_json(boot_id, started)), None),
            "logs" => response(id, true, Some(log_ring_json()), None),
            "timeline" => response(id, true, Some(timeline_json(boot_id, started)), None),
            "diagnostics" => response(id, true, Some(diagnostics_json(boot_id, started)), None),
            "magik" => match magik_control(args) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => operation_failure_response(id, &err),
            },
            "ui_test" => match ui_test_control(args) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => operation_failure_response(id, &err),
            },
            "sd_list_dir" => match sd_list_dir(args, request_received_monotonic_us) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => operation_failure_response(id, &err),
            },
            "sd_list_dir_v2" => match sd_list_dir_v2(args, request_received_monotonic_us) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => operation_failure_response(id, &err),
            },
            "sd_stat_item_v1" => match sd_stat_item(args) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => operation_failure_response(id, &err),
            },
            "sd_parse_mra_v1" => match sd_parse_mra(args) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => operation_failure_response(id, &err),
            },
            "framebuffer_capture" => {
                match framebuffer_capture(request_received, request_received_monotonic_us, started)
                {
                    Ok(result) => response(id, true, Some(result), None),
                    Err(err) => operation_failure_response(id, &err),
                }
            }
            "launcher_automation_begin" => match crate::launcher_automation::begin(args) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => operation_failure_response(id, &err),
            },
            "launcher_automation_request" => match crate::launcher_automation::request(args) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => operation_failure_response(id, &err),
            },
            "alpha_candidate_install" => match crate::alpha_candidate::install(args) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => alpha_candidate_failure_response(id, &err),
            },
            "reboot" => match schedule_reboot(args) {
                Ok(mode) => response(
                    id,
                    true,
                    Some(json!({"scheduled": true, "mode": mode})),
                    None,
                ),
                Err(err) => operation_failure_response(id, &err),
            },
            _ => failure_response(
                id,
                "unknown cmd",
                mister_magik_agent_protocol::FailureCode::UnknownCommand,
                mister_magik_agent_protocol::FailurePhase::Request,
                mister_magik_agent_protocol::RetryPolicy::Never,
                false,
            ),
        }
    }

    fn response(id: Option<Value>, ok: bool, result: Option<Value>, error: Option<&str>) -> String {
        let value = if ok {
            json!({"id": id.unwrap_or(Value::Null), "ok": true, "result": result.unwrap_or(Value::Null)})
        } else {
            json!({"id": id.unwrap_or(Value::Null), "ok": false, "error": error.unwrap_or("error")})
        };
        value.to_string()
    }

    pub(super) fn failure_response(
        id: Option<Value>,
        error: &str,
        code: mister_magik_agent_protocol::FailureCode,
        phase: mister_magik_agent_protocol::FailurePhase,
        retry_policy: mister_magik_agent_protocol::RetryPolicy,
        recovery_required: bool,
    ) -> String {
        let failure = mister_magik_agent_protocol::FailureMetadata {
            code,
            detail: error.to_owned(),
            phase,
            retry_policy,
            recovery_required,
        };
        json!({
            "id": id.unwrap_or(Value::Null),
            "ok": false,
            "error": error,
            "failure": failure.to_value(),
        })
        .to_string()
    }

    pub(super) fn authentication_failure_response(id: Option<Value>) -> String {
        failure_response(
            id,
            "unauthorized",
            mister_magik_agent_protocol::FailureCode::AuthenticationRequired,
            mister_magik_agent_protocol::FailurePhase::Authentication,
            mister_magik_agent_protocol::RetryPolicy::Never,
            false,
        )
    }

    pub(super) fn busy_failure_response(id: Option<Value>, error: &str) -> String {
        failure_response(
            id,
            error,
            mister_magik_agent_protocol::FailureCode::DeviceBusy,
            mister_magik_agent_protocol::FailurePhase::Availability,
            mister_magik_agent_protocol::RetryPolicy::Retry,
            false,
        )
    }

    pub(super) fn unavailable_failure_response(id: Option<Value>, error: &str) -> String {
        failure_response(
            id,
            error,
            mister_magik_agent_protocol::FailureCode::DeviceUnavailable,
            mister_magik_agent_protocol::FailurePhase::Availability,
            mister_magik_agent_protocol::RetryPolicy::Retry,
            false,
        )
    }

    pub(super) fn operation_failure_response(id: Option<Value>, error: &str) -> String {
        failure_response(
            id,
            error,
            mister_magik_agent_protocol::FailureCode::OperationFailed,
            mister_magik_agent_protocol::FailurePhase::Operation,
            mister_magik_agent_protocol::RetryPolicy::Never,
            false,
        )
    }

    pub(super) fn alpha_candidate_failure_response(
        id: Option<Value>,
        error: &crate::alpha_candidate::InstallFailure,
    ) -> String {
        use crate::alpha_candidate::InstallFailureKind;
        let (code, phase, retry_policy, recovery_required) = match error.kind {
            InstallFailureKind::InvalidRequest => (
                mister_magik_agent_protocol::FailureCode::InvalidRequest,
                mister_magik_agent_protocol::FailurePhase::Request,
                mister_magik_agent_protocol::RetryPolicy::Never,
                false,
            ),
            InstallFailureKind::OperationFailed => (
                mister_magik_agent_protocol::FailureCode::OperationFailed,
                mister_magik_agent_protocol::FailurePhase::Operation,
                mister_magik_agent_protocol::RetryPolicy::Never,
                false,
            ),
            InstallFailureKind::ArtifactMismatch => (
                mister_magik_agent_protocol::FailureCode::ArtifactMismatch,
                mister_magik_agent_protocol::FailurePhase::Artifact,
                mister_magik_agent_protocol::RetryPolicy::ReconcileThenRetry,
                false,
            ),
            InstallFailureKind::RecoveryRequired => (
                mister_magik_agent_protocol::FailureCode::RecoveryRequired,
                mister_magik_agent_protocol::FailurePhase::Recovery,
                mister_magik_agent_protocol::RetryPolicy::OperatorRequired,
                true,
            ),
        };
        failure_response(
            id,
            &error.detail,
            code,
            phase,
            retry_policy,
            recovery_required,
        )
    }

    fn attach_io_operation_evidence(result: &mut Value, evidence: Value) {
        if let Some(result) = result.as_object_mut() {
            result.insert("io_operation".to_string(), evidence);
        }
    }

    fn peak_rss_kb() -> Option<u64> {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: getrusage initializes the supplied rusage value on success.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
            return None;
        }
        // SAFETY: getrusage returned success, so usage is initialized.
        let usage = unsafe { usage.assume_init() };
        u64::try_from(usage.ru_maxrss).ok()
    }

    fn local_ipv5() -> Option<String> {
        let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("192.0.2.1:9").ok()?;
        Some(socket.local_addr().ok()?.ip().to_string())
    }

    fn status_json(boot_id: u64, started: Instant) -> Value {
        json!({
            "agent": {
                "version": env!("CARGO_PKG_VERSION"),
                "agent_version": mister_magik_agent_protocol::AGENT_VERSION,
                "protocol_version": mister_magik_agent_protocol::PROTOCOL_VERSION,
                "capabilities": [
                    mister_magik_agent_protocol::FRAMEBUFFER_CAPTURE_CAPABILITY,
                    mister_magik_agent_protocol::DEVICE_TELEMETRY_CAPABILITY,
                    mister_magik_agent_protocol::LAUNCHER_AUTOMATION_CAPABILITY,
                    mister_magik_agent_protocol::UI_TEST_SESSION_CAPABILITY,
                    mister_magik_agent_protocol::ALPHA_CANDIDATE_INSTALL_CAPABILITY,
                    mister_magik_agent_protocol::RUNTIME_UPLOAD_CAPABILITY,
                ],
                "boot_id": boot_id,
                "uptime_ms": started.elapsed().as_millis() as u64,
                "port": AGENT_PORT,
            },
            "network": {
                "interface": IFACE,
                "ip": local_ipv5(),
                "carrier": read_trimmed("/sys/class/net/eth0/carrier"),
                "operstate": read_trimmed("/sys/class/net/eth0/operstate"),
                "mac": read_trimmed("/sys/class/net/eth0/address"),
                "stats": read_netdev_stats_value(IFACE),
                "routes": read_routes(),
                "arp": read_arp_entries(),
            },
            "processes": {
                "sshd": read_pid_list("sshd"),
                "MiSTer_MagiKDev": read_pid_list("MiSTer_MagiKDev"),
                "MiSTer_MagiK": read_pid_list("MiSTer_MagiK"),
                "mister-magik-fb": read_pid_list("mister-magik-fb"),
            },
            "system": {
                "uptime": read_trimmed("/proc/uptime"),
            },
            "scanout_slots": scanout_slots_status_json(),
        })
    }

    fn scanout_slots_status_json() -> Value {
        json!({
            "module_loaded": Path::new("/sys/module/mister_magik_scanout_slots").exists(),
            "device_ready": Path::new("/dev/mister-magik-scanout-slots").exists(),
        })
    }

    fn diagnostics_json(boot_id: u64, started: Instant) -> Value {
        json!({
            "schema": "mister-magik-agent-diagnostics-v1",
            "collected_uptime_ms": uptime_ms_now(),
            "status": status_json(boot_id, started),
            "timeline": timeline_json(boot_id, started),
            "agent_logs": log_ring_json(),
            "net": {
                "carrier": read_text_value("/sys/class/net/eth0/carrier"),
                "operstate": read_text_value("/sys/class/net/eth0/operstate"),
                "address": read_text_value("/sys/class/net/eth0/address"),
                "route": read_text_value("/proc/net/route"),
                "arp": read_text_value("/proc/net/arp"),
                "dev": read_text_value("/proc/net/dev"),
            },
            "processes": {
                "ps": command_text_value("ps", &["w"]),
                "sshd": read_pid_list("sshd"),
                "MiSTer_MagiKDev": read_pid_list("MiSTer_MagiKDev"),
                "MiSTer_MagiK": read_pid_list("MiSTer_MagiK"),
                "mister-magik-fb": read_pid_list("mister-magik-fb"),
            },
            "files": {
                "slint_status": read_text_value("/tmp/mister-magik/status.json"),
                "main_status": read_text_value("/tmp/mister-magik/main-status.json"),
                "events_tail": tail_text_value("/tmp/mister-magik/events.jsonl", 80),
                "slint_log_tail": tail_text_value("/tmp/mister-magik-slint.log", 120),
                "main_log_tail": tail_text_value("/tmp/mister-magik-main.log", 120),
                "agent_tmp_log_tail": tail_text_value(LOG, 160),
                "agent_persistent_log_tail": tail_text_value(PLOG, 160),
                "boot_analytics_tail": tail_text_value("/tmp/mister-magik-boot-analytics.tsv", 80),
                "scanout_proc_modules": tail_text_value("/proc/modules", 80),
            },
            "crashes": crash_reports_json(),
            "catalog_failures": catalog_failure_reports_json(),
            "catalog_progress": latest_diagnostic_report(
                &CATALOG_PROGRESS_PATHS,
                "updated_unix_ms",
            ),
            "latch_failure": current_latch_failure_report(),
            "fpga_video_diagnostics": fpga_video_diagnostics_json(),
        })
    }

    fn video_diagnostics_state_name(
        state: mister_magik_video_diagnostics_contract::VideoDiagnosticsState,
    ) -> &'static str {
        use mister_magik_video_diagnostics_contract::VideoDiagnosticsState;
        match state {
            VideoDiagnosticsState::Idle => "idle",
            VideoDiagnosticsState::Armed => "armed",
            VideoDiagnosticsState::Frozen => "frozen",
            VideoDiagnosticsState::Partial => "partial",
        }
    }

    fn fpga_video_diagnostics_unavailable(reason: impl Into<String>) -> Value {
        json!({
            "schema": "mister-magik-fpga-video-diagnostics-v1",
            "available": false,
            "coherent": false,
            "classification": "unclassified",
            "reason": reason.into(),
        })
    }

    fn fpga_video_diagnostics_json() -> Value {
        let capture_start_monotonic_us = uptime_ms_now().saturating_mul(1_000);
        let main_before = read_json_value("/tmp/mister-magik/main-status.json");
        if main_before.get("launcher_state").and_then(Value::as_str) != Some("LauncherActive") {
            return fpga_video_diagnostics_unavailable(
                "diagnostic readout requires stable LauncherActive ownership",
            );
        }
        let owner_epoch_before = main_before.get("fpga_owner_epoch").and_then(Value::as_u64);
        let mut fpga = match FpgaIo::open() {
            Ok(fpga) => fpga,
            Err(error) => {
                return fpga_video_diagnostics_unavailable(format!("open FPGA IO: {error}"));
            }
        };
        let _uio_guard = match fpga.lock_uio_transaction() {
            Ok(guard) => guard,
            Err(error) => {
                return fpga_video_diagnostics_unavailable(format!(
                    "lock FPGA UIO transaction: {error}"
                ));
            }
        };
        let latch_status = match fpga.read_latched_fbuf_status() {
            Ok(status)
                if status.flags & (1 << mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP)
                    != 0 =>
            {
                status
            }
            Ok(_) => {
                return fpga_video_diagnostics_unavailable(
                    "diagnostic readout requires active MagiK FPGA ownership",
                );
            }
            Err(error) => {
                return fpga_video_diagnostics_unavailable(format!(
                    "read FPGA latch ownership: {error}"
                ));
            }
        };
        let Some(latch_capabilities) = fpga.latch_capabilities else {
            return fpga_video_diagnostics_unavailable(
                "latch capabilities were not retained after status negotiation",
            );
        };
        let latch_status_json = Some(json!({
            "active_sequence": latch_status.active_sequence,
            "pending_sequence": latch_status.pending_sequence,
            "flags": latch_status.flags,
            "flip_count": latch_status.flip_count,
            "post_count": latch_status.post_count,
            "drop_count": latch_status.drop_count,
            "active_base": latch_status.active_base,
            "active_width": latch_status.active_width,
            "active_height": latch_status.active_height,
            "active_stride": latch_status.active_stride,
            "reject_count": latch_status.reject_count,
            "active_route_epoch": latch_status.active_route_epoch,
            "accepted_sequence": latch_status.accepted_sequence,
            "active_transaction": latch_status.active_transaction,
            "pending_transaction": latch_status.pending_transaction,
            "accepted_transaction": latch_status.accepted_transaction,
            "crc": latch_status.crc,
        }));
        let (readout, passive_observer_probe_error) = match fpga.read_video_diagnostics() {
            Ok(readout) => (Some(readout), None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                (None, Some(error.to_string()))
            }
            Err(error) => {
                return fpga_video_diagnostics_unavailable(format!(
                    "read passive FPGA video diagnostics: {error}"
                ));
            }
        };
        let repair_telemetry = if readout.is_none() {
            match fpga.read_presentation_telemetry() {
                Ok(telemetry) => Some(telemetry),
                Err(error) => {
                    return fpga_video_diagnostics_unavailable(format!(
                        "read repair-only latch telemetry: {error}"
                    ));
                }
            }
        } else {
            None
        };
        let main_after = read_json_value("/tmp/mister-magik/main-status.json");
        let owner_epoch_after = main_after.get("fpga_owner_epoch").and_then(Value::as_u64);
        let launcher_state_stable =
            main_after.get("launcher_state").and_then(Value::as_str) == Some("LauncherActive");
        let (latch_ownership_stable, ownership_check_error) = match fpga.read_latched_fbuf_status()
        {
            Ok(status) => (
                Some(
                    status.flags & (1 << mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP) != 0,
                ),
                None,
            ),
            Err(error) => (None, Some(error.to_string())),
        };
        let owner_stable = owner_epoch_before.is_some()
            && owner_epoch_before == owner_epoch_after
            && launcher_state_stable
            && latch_ownership_stable == Some(true);
        let capture_end_monotonic_us = uptime_ms_now().saturating_mul(1_000);
        match (readout, repair_telemetry) {
            (Some(readout), _) => readout.to_json(VideoDiagnosticsJsonContext {
                owner_stable,
                latch_ownership_stable,
                launcher_state_stable,
                ownership_check_error,
                owner_epoch_before,
                owner_epoch_after,
                latch_status_json,
                capture_start_monotonic_us,
                capture_end_monotonic_us,
            }),
            (None, Some(telemetry)) => {
                let coherent = owner_stable
                    && telemetry.magik_ownership()
                    && telemetry.lifetime_invariant_valid();
                json!({
                    "schema": "mister-magik-fpga-video-diagnostics-v2",
                    "diagnostic_architecture": "unverified-observer-fallback-v1",
                    "available": true,
                    "coherent": coherent,
                    "classification": if coherent {
                        "observer_fallback_transport_ready"
                    } else {
                        "observer_fallback_transport_incoherent"
                    },
                    "sink_visibility": "unobserved",
                    "capture_start_monotonic_us": capture_start_monotonic_us,
                    "capture_end_monotonic_us": capture_end_monotonic_us,
                    "owner_epoch_before": owner_epoch_before,
                    "owner_epoch_after": owner_epoch_after,
                    "passive_observer_probe_error": passive_observer_probe_error,
                    "coherence": {
                        "latch_ownership_stable": latch_ownership_stable,
                        "launcher_state_stable": launcher_state_stable,
                        "ownership_check_error": ownership_check_error,
                    },
                    "capabilities": {
                        "passive_video_observer": false,
                        "protocol_version": latch_capabilities.protocol_version,
                        "flags": latch_capabilities.flags,
                        "max_width": latch_capabilities.max_width,
                        "max_height": latch_capabilities.max_height,
                        "max_stride_bytes": latch_capabilities.max_stride_bytes,
                        "crc": latch_capabilities.crc,
                    },
                    "latch_status": latch_status_json,
                    "presentation_telemetry": {
                        "owned_vblank_count": telemetry.owned_vblank_count,
                        "presented_vblank_count": telemetry.presented_vblank_count,
                        "repeated_vblank_count": telemetry.repeated_vblank_count,
                        "ownership_loss_count": telemetry.ownership_loss_count,
                        "active_sequence": telemetry.active_sequence,
                        "flags": telemetry.flags,
                        "magik_ownership": telemetry.magik_ownership(),
                        "lifetime_invariant_valid": telemetry.lifetime_invariant_valid(),
                        "crc": telemetry.crc,
                    },
                })
            }
            (None, None) => fpga_video_diagnostics_unavailable(
                "repair-only diagnostics did not produce presentation telemetry",
            ),
        }
    }

    fn ui_test_control(args: Value) -> Result<Value, String> {
        let request = mister_magik_agent_protocol::UiTestCaseRequest::from_value(&args)?;
        Ok(json!({
            "schema": mister_magik_agent_protocol::UI_TEST_REQUEST_SCHEMA,
            "case": request.case,
            "fixture": request.fixture,
            "timeout_ms": request.timeout_ms,
            "effects": {
                "allow_core_launch": false,
                "allow_catalog_write": false,
                "allow_reboot": false,
            },
            "accepted": true,
        }))
    }

    fn magik_control(args: Value) -> Result<Value, String> {
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("status");
        match action {
            "status" => Ok(magik_status_json(action, None, None)),
            "suspend" | "resume" | "restart-launcher" | "return-to-launcher" | "launch"
            | "exit-to-menu" => magik_acknowledged_action(action, &args),
            _ => Err(format!("unsupported magik action: {action}")),
        }
    }

    fn sd_list_dir(args: Value, request_received_monotonic_us: u64) -> Result<Value, String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(crate::sd_browse::ROOT_PATH);
        let show_hidden = args
            .get("show_hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let operation_started_monotonic_us = monotonic_us_now();
        let mut result = crate::sd_browse::list_dir_at_root(
            Path::new(crate::sd_browse::SD_ROOT),
            path,
            show_hidden,
        )?;
        attach_directory_io_evidence(
            &mut result,
            "directory_list_v1",
            request_received_monotonic_us,
            operation_started_monotonic_us,
        );
        Ok(result)
    }

    fn sd_list_dir_v2(args: Value, request_received_monotonic_us: u64) -> Result<Value, String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(crate::sd_browse::ROOT_PATH);
        let show_hidden = args
            .get("show_hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let operation_started_monotonic_us = monotonic_us_now();
        let mut result = crate::sd_browse::list_dir_fast_at_root(
            Path::new(crate::sd_browse::SD_ROOT),
            path,
            show_hidden,
        )?;
        attach_directory_io_evidence(
            &mut result,
            "directory_list_v2",
            request_received_monotonic_us,
            operation_started_monotonic_us,
        );
        Ok(result)
    }

    fn attach_directory_io_evidence(
        result: &mut Value,
        operation: &str,
        request_received_monotonic_us: u64,
        operation_started_monotonic_us: u64,
    ) {
        let operation_ended_monotonic_us = monotonic_us_now();
        let serialized_bytes = result["io_counts"]["serialized_bytes"]
            .as_u64()
            .unwrap_or(0);
        let phases = result
            .get("io_phases_us")
            .cloned()
            .unwrap_or_else(|| json!({}));
        attach_io_operation_evidence(
            result,
            io_operation_evidence(IoOperationEvidence {
                operation,
                request_received_monotonic_us,
                operation_started_monotonic_us,
                operation_ended_monotonic_us,
                bytes_read: 0,
                bytes_written: serialized_bytes,
                peak_buffer_ownership_bytes: serialized_bytes,
                peak_rss_kb: peak_rss_kb(),
                phases_us: phases,
            }),
        );
    }

    fn sd_stat_item(args: Value) -> Result<Value, String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(crate::sd_browse::ROOT_PATH);
        crate::sd_browse::stat_item_at_root(Path::new(crate::sd_browse::SD_ROOT), path)
    }

    fn sd_parse_mra(args: Value) -> Result<Value, String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(crate::sd_browse::ROOT_PATH);
        crate::sd_browse::parse_mra_at_root(Path::new(crate::sd_browse::SD_ROOT), path)
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct FramebufferGeometry {
        width: usize,
        height: usize,
        stride: usize,
        bpp: usize,
    }

    impl FramebufferGeometry {
        fn bytes(self) -> Result<usize, String> {
            self.stride
                .checked_mul(self.height)
                .ok_or_else(|| "framebuffer byte size overflow".to_string())
        }
    }

    struct RawFramebufferCapture {
        result: Value,
        payload: Vec<u8>,
    }

    struct FramebufferRead {
        raw: Vec<u8>,
        geometry: FramebufferGeometry,
        source: FramebufferCaptureSource,
    }

    enum FramebufferCaptureSource {
        Fb0,
        ProducerComposition {
            sequence: u64,
            authoritative_error: String,
        },
        FpgaLatchedScanoutSlots {
            active_base: u32,
            active_sequence: u16,
            pending_sequence: u16,
            flags: u16,
            flip_count: u16,
            post_count: u16,
            drop_count: u16,
            region_index: usize,
            region_name: String,
        },
    }

    impl FramebufferCaptureSource {
        fn label(&self) -> &'static str {
            match self {
                Self::Fb0 => "fb0",
                Self::ProducerComposition { .. } => "producer-composition",
                Self::FpgaLatchedScanoutSlots { .. } => "fpga-latched-scanout-slots",
            }
        }

        fn authoritative_scanout(&self) -> bool {
            matches!(self, Self::FpgaLatchedScanoutSlots { .. })
        }

        fn json(&self) -> Value {
            match self {
                Self::Fb0 => json!({"kind": self.label()}),
                Self::ProducerComposition {
                    sequence,
                    authoritative_error,
                } => json!({
                    "kind": self.label(),
                    "sequence": sequence,
                    "authoritative_error": authoritative_error,
                }),
                Self::FpgaLatchedScanoutSlots {
                    active_base,
                    active_sequence,
                    pending_sequence,
                    flags,
                    flip_count,
                    post_count,
                    drop_count,
                    region_index,
                    region_name,
                } => json!({
                    "kind": self.label(),
                    "active_base": format!("0x{active_base:08x}"),
                    "active_sequence": active_sequence,
                    "pending_sequence": pending_sequence,
                    "flags": format!("0x{flags:04x}"),
                    "flip_count": flip_count,
                    "post_count": post_count,
                    "drop_count": drop_count,
                    "region_index": region_index,
                    "region_name": region_name,
                }),
            }
        }
    }

    fn framebuffer_capture(
        request_received: Instant,
        request_received_monotonic_us: u64,
        started: Instant,
    ) -> Result<Value, String> {
        let operation_started_monotonic_us = monotonic_us_now();
        let start = Instant::now();
        let request_received_uptime_ms =
            request_received.duration_since(started).as_millis() as u64;
        let dispatch_us = elapsed_us(request_received);
        let read_t = Instant::now();
        let capture = read_framebuffer_capture()?;
        let raw_read_us = elapsed_us(read_t);
        let geometry = capture.geometry;
        let raw = capture.raw;
        let source = capture.source;
        let geometry_us = 0;
        let source_json = source.json();
        let source_label = source.label();
        let content_scan_t = Instant::now();
        let (content_nonzero_bytes, content_varied) = framebuffer_content_stats(&raw, geometry);
        let content_scan_us = elapsed_us(content_scan_t);
        let png_t = Instant::now();
        let png = framebuffer_png(&raw, geometry)?;
        let png_total_us = elapsed_us(png_t);
        let hex_t = Instant::now();
        let png_hex = encode_hex(&png.bytes);
        let hex_encode_us = elapsed_us(hex_t);
        let total_us = elapsed_us(start);
        let operation_ended_monotonic_us = monotonic_us_now();
        let raw_bytes = raw.len() as u64;
        let png_bytes = png.bytes.len() as u64;
        let png_hex_bytes = png_hex.len() as u64;
        let peak_buffer_ownership_bytes = raw_bytes.saturating_add(png.workspace_peak_bytes).max(
            raw_bytes
                .saturating_add(png_bytes)
                .saturating_add(png_hex_bytes),
        );
        let phases = json!({
            "raw_framebuffer_capture": raw_read_us,
            "content_scan": content_scan_us,
            "rgb_conversion": png.timing.rgba_convert_us,
            "zlib": png.timing.zlib_encode_us,
            "crc": png.timing.crc32_us,
            "png_wrap": png.timing.png_wrap_us,
            "hex_encoding": hex_encode_us,
        });
        let mut result = json!({
            "schema": "mister-magik-framebuffer-capture-v2",
            "source": source_label,
            "capture_source": source_json,
            "authoritative_scanout": source.authoritative_scanout(),
            "width": geometry.width,
            "height": geometry.height,
            "stride": geometry.stride,
            "bpp": geometry.bpp,
            "raw_bytes": raw.len(),
            "rgba_bytes": logical_rgba_len(geometry)?,
            "png_bytes": png.bytes.len(),
            "png_hex_bytes": png_hex.len(),
            "png_hex": png_hex,
            "content_nonzero_bytes": content_nonzero_bytes,
            "content_varied": content_varied,
            "elapsed_ms": total_us / 1000,
            "timings": {
                "request_received_uptime_ms": request_received_uptime_ms,
                "dispatch_us": dispatch_us,
                "geometry_us": geometry_us,
                "raw_read_us": raw_read_us,
                "content_scan_us": content_scan_us,
                "rgba_convert_us": png.timing.rgba_convert_us,
                "zlib_encode_us": png.timing.zlib_encode_us,
                "crc32_us": png.timing.crc32_us,
                "png_wrap_us": png.timing.png_wrap_us,
                "png_total_us": png_total_us,
                "hex_encode_us": hex_encode_us,
                "total_us": total_us,
            },
        });
        attach_io_operation_evidence(
            &mut result,
            io_operation_evidence(IoOperationEvidence {
                operation: "framebuffer_png_capture",
                request_received_monotonic_us,
                operation_started_monotonic_us,
                operation_ended_monotonic_us,
                bytes_read: raw_bytes,
                bytes_written: png_hex_bytes,
                peak_buffer_ownership_bytes,
                peak_rss_kb: peak_rss_kb(),
                phases_us: phases,
            }),
        );
        Ok(result)
    }

    fn framebuffer_content_stats(raw: &[u8], geometry: FramebufferGeometry) -> (usize, bool) {
        let bytes_per_pixel = (geometry.bpp / 8).max(1);
        let active_row_bytes = geometry.width.saturating_mul(bytes_per_pixel);
        let mut nonzero = 0usize;
        let mut first_pixel: Option<&[u8]> = None;
        let mut varied = false;
        for row in raw.chunks(geometry.stride).take(geometry.height) {
            let active = &row[..active_row_bytes.min(row.len())];
            nonzero = nonzero.saturating_add(active.iter().filter(|byte| **byte != 0).count());
            for pixel in active.chunks_exact(bytes_per_pixel).step_by(16) {
                match first_pixel {
                    Some(first) if first != pixel => varied = true,
                    None => first_pixel = Some(pixel),
                    Some(_) => {}
                }
            }
        }
        (nonzero, varied)
    }

    fn read_framebuffer_capture() -> Result<FramebufferRead, String> {
        select_framebuffer_capture(
            Path::new(SCANOUT_SLOTS_DEVICE).exists(),
            launcher_in_compatibility_mode(),
            read_fpga_latched_scanout_slots_capture,
            read_producer_composition_capture,
            read_fb0_capture,
        )
    }

    fn launcher_in_compatibility_mode() -> bool {
        let status = read_json_value("/tmp/mister-magik/status.json");
        status.get("present_backend").and_then(Value::as_str) == Some("compatibility-fb0")
            || status.get("present_status").and_then(Value::as_str) == Some("compatibility")
    }

    fn read_producer_composition_capture(
        authoritative_error: &str,
    ) -> Result<FramebufferRead, String> {
        let mut stream = TcpStream::connect(("127.0.0.1", FRAMEBUFFER_PRODUCER_PORT))
            .map_err(|error| format!("connect producer stream: {error}"))?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("producer stream timed out before keyframe".to_string());
            }
            stream
                .set_read_timeout(Some(remaining))
                .map_err(|error| format!("set producer stream timeout: {error}"))?;
            let (header, payload) =
                read_frame(&mut stream).map_err(|error| format!("read producer frame: {error}"))?;
            match header.kind {
                FrameKind::Heartbeat | FrameKind::Hello | FrameKind::RectDelta => continue,
                FrameKind::End => return Err("producer stream ended before keyframe".to_string()),
                FrameKind::Error => {
                    return Err("producer stream reported an error before keyframe".to_string());
                }
                FrameKind::Keyframe => {
                    if header.flags != FLAG_LZ4_SIZE_PREPENDED {
                        return Err(format!(
                            "producer keyframe has unsupported flags 0x{:04x}",
                            header.flags
                        ));
                    }
                    let expected = usize::try_from(header.raw_bytes)
                        .map_err(|_| "producer keyframe raw size overflow".to_string())?;
                    let raw = mister_magik_agent_protocol::decompress_size_prepended_exact(
                        &payload,
                        expected,
                        MAX_FRAME_SURFACE_BYTES,
                    )?;
                    let width = usize::try_from(header.geometry.width)
                        .map_err(|_| "producer keyframe width overflow".to_string())?;
                    let height = usize::try_from(header.geometry.height)
                        .map_err(|_| "producer keyframe height overflow".to_string())?;
                    let stride = width
                        .checked_mul(2)
                        .ok_or_else(|| "producer keyframe stride overflow".to_string())?;
                    return Ok(FramebufferRead {
                        raw,
                        geometry: FramebufferGeometry {
                            width,
                            height,
                            stride,
                            bpp: 16,
                        },
                        source: FramebufferCaptureSource::ProducerComposition {
                            sequence: header.sequence,
                            authoritative_error: authoritative_error.to_string(),
                        },
                    });
                }
            }
        }
    }

    fn read_fb0_capture() -> Result<FramebufferRead, String> {
        let geometry = framebuffer_geometry()?;
        let expected = geometry.bytes()?;
        let mut raw = vec![0u8; expected];
        let mut fb0 = File::open("/dev/fb0").map_err(|err| format!("open /dev/fb0: {err}"))?;
        fb0.read_exact(&mut raw)
            .map_err(|err| format!("read /dev/fb0: {err}"))?;
        Ok(FramebufferRead {
            raw,
            geometry,
            source: FramebufferCaptureSource::Fb0,
        })
    }

    #[derive(Clone, Debug)]
    struct ScanoutSlotsRegion {
        index: usize,
        name: String,
        phys: u32,
        len: usize,
        mmap_offset_bytes: usize,
    }

    #[derive(Clone, Copy, Debug)]
    struct LatchedFbufStatus {
        active_sequence: u16,
        pending_sequence: u16,
        flags: u16,
        flip_count: u16,
        post_count: u16,
        drop_count: u16,
        active_base: u32,
        active_width: u16,
        active_height: u16,
        active_stride: u16,
        reject_count: u16,
        active_route_epoch: u16,
        accepted_sequence: u16,
        active_transaction: u16,
        pending_transaction: u16,
        accepted_transaction: u16,
        crc: Option<u16>,
    }

    impl LatchedFbufStatus {
        fn supported(self) -> bool {
            self.active_width > 0
                && self.active_height > 0
                && self.active_stride >= self.active_width.saturating_mul(2)
        }
    }

    fn read_fpga_latched_scanout_slots_capture() -> Result<FramebufferRead, String> {
        if !Path::new(SCANOUT_SLOTS_DEVICE).exists() {
            return Err("scanout slots device is not present".to_string());
        }
        let mut fpga = FpgaIo::open().map_err(|err| format!("open FPGA IO: {err}"))?;
        let _uio_guard = fpga
            .lock_uio_transaction()
            .map_err(|err| format!("lock FPGA UIO transaction: {err}"))?;
        let status = fpga
            .read_latched_fbuf_status()
            .map_err(|err| format!("read latched fbuf status: {err}"))?;
        if !status.supported() {
            return Err("latched framebuffer status is not active".to_string());
        }
        let geometry = FramebufferGeometry {
            width: status.active_width as usize,
            height: status.active_height as usize,
            stride: status.active_stride as usize,
            bpp: 16,
        };
        let expected = geometry.bytes()?;
        let region = scanout_slots_region_for_phys(status.active_base, expected)?;
        let raw = read_scanout_slots_region_raw(&region, expected)?;
        Ok(FramebufferRead {
            raw,
            geometry,
            source: FramebufferCaptureSource::FpgaLatchedScanoutSlots {
                active_base: status.active_base,
                active_sequence: status.active_sequence,
                pending_sequence: status.pending_sequence,
                flags: status.flags,
                flip_count: status.flip_count,
                post_count: status.post_count,
                drop_count: status.drop_count,
                region_index: region.index,
                region_name: region.name,
            },
        })
    }

    fn scanout_slots_region_for_phys(
        active_base: u32,
        required_len: usize,
    ) -> Result<ScanoutSlotsRegion, String> {
        let layout = read_scanout_slots_layout()?;
        for (index, slot) in layout.slots.iter().enumerate() {
            let region = ScanoutSlotsRegion {
                index,
                name: format!("hidden-slot-{}", index + 1),
                phys: slot.physical_address,
                len: layout.map_bytes as usize,
                mmap_offset_bytes: slot.mmap_offset_bytes as usize,
            };
            if region.phys != active_base {
                continue;
            }
            if region.len < required_len {
                return Err(format!(
                    "scanout slot {} has {} bytes, need {required_len}",
                    region.name, region.len
                ));
            }
            return Ok(region);
        }
        Err(format!(
            "no scanout slot matches active base 0x{active_base:08x}"
        ))
    }

    fn read_scanout_slots_layout() -> Result<ScanoutSlotsLayout, String> {
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(SCANOUT_SLOTS_DEVICE)
            .map_err(|err| format!("open {SCANOUT_SLOTS_DEVICE}: {err}"))?;
        let mut layout = ScanoutSlotsLayout::default();
        let result = unsafe {
            ioctl(
                device.as_raw_fd(),
                SCANOUT_SLOTS_GET_LAYOUT as c_ulong,
                &mut layout,
            )
        };
        if result != 0 {
            return Err(format!(
                "get scanout slots layout: {}",
                io::Error::last_os_error()
            ));
        }
        if layout != EXPECTED_LAYOUT {
            return Err(format!(
                "scanout slots layout mismatch: expected {EXPECTED_LAYOUT:?}, got {layout:?}"
            ));
        }
        Ok(layout)
    }

    fn read_scanout_slots_region_raw(
        region: &ScanoutSlotsRegion,
        frame_len: usize,
    ) -> Result<Vec<u8>, String> {
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(SCANOUT_SLOTS_DEVICE)
            .map_err(|err| format!("open {SCANOUT_SLOTS_DEVICE}: {err}"))?;
        let mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                region.len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                device.as_raw_fd(),
                region.mmap_offset_bytes as libc::off_t,
            )
        };
        if mem == libc::MAP_FAILED {
            return Err(format!(
                "mmap scanout slot {}: {}",
                region.name,
                io::Error::last_os_error()
            ));
        }
        if mem.is_null() {
            return Err(format!("mmap scanout slot {} returned null", region.name));
        }
        let raw = unsafe { std::slice::from_raw_parts(mem.cast::<u8>(), frame_len).to_vec() };
        unsafe {
            libc::munmap(mem, region.len);
        }
        Ok(raw)
    }

    struct LegacyVideoDiagnosticsReadout {
        control_before: mister_magik_video_diagnostics_contract::VideoDiagnosticsControlSnapshot,
        avalon: mister_magik_video_diagnostics_contract::VideoDiagnosticsAvalonSnapshot,
        output: mister_magik_video_diagnostics_contract::VideoDiagnosticsOutputSnapshot,
        control_after: mister_magik_video_diagnostics_contract::VideoDiagnosticsControlSnapshot,
    }

    struct VideoDiagnosticsJsonContext {
        owner_stable: bool,
        latch_ownership_stable: Option<bool>,
        launcher_state_stable: bool,
        ownership_check_error: Option<String>,
        owner_epoch_before: Option<u64>,
        owner_epoch_after: Option<u64>,
        latch_status_json: Option<Value>,
        capture_start_monotonic_us: u64,
        capture_end_monotonic_us: u64,
    }

    enum VideoDiagnosticsReadout {
        ScalerFetchLiveness(ScalerFetchLivenessDiagnosticsReadout),
        RawScaler(RawScalerDiagnosticsReadout),
        HdmiLock(HdmiLockDiagnosticsReadout),
        Legacy(Box<LegacyVideoDiagnosticsReadout>),
    }

    struct ScalerFetchLivenessDiagnosticsReadout {
        samples: [mister_magik_video_diagnostics_contract::ScalerFetchLivenessState; 3],
        sample_interval_us: [u64; 2],
    }

    struct RawScalerDiagnosticsReadout {
        samples: [mister_magik_video_diagnostics_contract::RawScalerState; 3],
        sample_interval_us: [u64; 2],
    }

    struct HdmiLockDiagnosticsReadout {
        lock: mister_magik_video_diagnostics_contract::HdmiEvidence,
        output_activity: Option<HdmiOutputActivityWindow>,
        path_activity: Option<HdmiPathActivityWindow>,
    }

    struct HdmiOutputActivityWindow {
        first: mister_magik_video_diagnostics_contract::HdmiOutputActivity,
        second: mister_magik_video_diagnostics_contract::HdmiOutputActivity,
        sample_interval_us: u64,
    }

    struct HdmiPathActivityWindow {
        final_first: mister_magik_video_diagnostics_contract::HdmiFinalPathActivity,
        final_second: mister_magik_video_diagnostics_contract::HdmiFinalPathActivity,
        raw_first: mister_magik_video_diagnostics_contract::HdmiScalerRawActivity,
        raw_second: mister_magik_video_diagnostics_contract::HdmiScalerRawActivity,
        post_first: mister_magik_video_diagnostics_contract::HdmiPostOsdActivity,
        post_second: mister_magik_video_diagnostics_contract::HdmiPostOsdActivity,
        avalon_first: mister_magik_video_diagnostics_contract::HdmiAvalonLivenessActivity,
        avalon_second: mister_magik_video_diagnostics_contract::HdmiAvalonLivenessActivity,
        scaler_fetch_first:
            Option<mister_magik_video_diagnostics_contract::HdmiScalerFetchActivity>,
        scaler_fetch_second:
            Option<mister_magik_video_diagnostics_contract::HdmiScalerFetchActivity>,
        sample_interval_us: u64,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) struct ThreeClassDeltas {
        pub(super) no_de: u8,
        pub(super) all_zero: u8,
        pub(super) nonzero: u8,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) struct FinalPathDeltas {
        pub(super) no_de: u8,
        pub(super) black_direct: u8,
        pub(super) black_scaled: u8,
        pub(super) black_mixed: u8,
        pub(super) nonzero: u8,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) struct AvalonLivenessDeltas {
        pub(super) bucket: u8,
        pub(super) request: u8,
        pub(super) accepted: u8,
        pub(super) returned: u8,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) struct ScalerFetchDeltas {
        pub(super) batch_two: u8,
        pub(super) starved_frame: u8,
    }

    fn sequences_advance(sequences: [u16; 3]) -> bool {
        sequences.windows(2).all(|pair| {
            let delta = pair[1].wrapping_sub(pair[0]);
            delta != 0 && delta <= 0x7fff
        })
    }

    fn nibble_sequences_advance(sequences: [u8; 3]) -> bool {
        sequences
            .windows(2)
            .all(|pair| pair[1] == (pair[0].wrapping_add(1) & 0x0f))
    }

    pub(super) fn scaler_fetch_liveness_classification(
        samples: [&mister_magik_video_diagnostics_contract::ScalerFetchLivenessState; 3],
    ) -> &'static str {
        use mister_magik_video_diagnostics_contract as contract;

        let flags = samples
            .iter()
            .fold(0, |flags, sample| flags | sample.flags());
        let invalid_flags = contract::SCALER_FETCH_LIVENESS_STATE_FLAG_OBSERVER_FAULT
            | contract::SCALER_FETCH_LIVENESS_STATE_FLAG_RESET_AMBIGUITY
            | contract::SCALER_FETCH_LIVENESS_STATE_FLAG_COUNTER_AMBIGUOUS;
        if !samples.iter().all(|sample| sample.record_valid())
            || flags & invalid_flags != 0
            || !nibble_sequences_advance(samples.map(|sample| sample.publication_sequence()))
        {
            return "scaler_fetch_liveness_evidence_inconclusive";
        }

        if samples.iter().any(|sample| sample.first_stall_valid()) {
            let first = samples[0];
            if !samples.iter().all(|sample| {
                sample.first_stall_valid()
                    && sample.frozen_cause() == first.frozen_cause()
                    && sample.frozen_return_phase() == first.frozen_return_phase()
                    && sample.frozen_fifo_depth() == first.frozen_fifo_depth()
                    && sample.frozen_address_fold() == first.frozen_address_fold()
            }) {
                return "scaler_fetch_liveness_evidence_inconclusive";
            }
            return match first.frozen_cause() {
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_NO_REQUEST_SEEN => {
                    "scaler_fetch_no_request_seen"
                }
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_ACCEPT_BLOCKED => {
                    "scaler_fetch_accept_blocked"
                }
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_FIRST_RETURN_MISSING => {
                    "scaler_fetch_first_return_missing"
                }
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_RETURN_INCOMPLETE => {
                    "scaler_fetch_return_incomplete"
                }
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_REQUEST_CANCELLED => {
                    "scaler_fetch_request_cancelled"
                }
                _ => "scaler_fetch_liveness_evidence_inconclusive",
            };
        }

        if samples.iter().all(|sample| sample.normal_liveness_seen()) {
            "scaler_fetch_normal_liveness"
        } else {
            "scaler_fetch_liveness_evidence_inconclusive"
        }
    }

    pub(super) fn raw_scaler_classification(
        samples: [&mister_magik_video_diagnostics_contract::RawScalerOrderedSignatureState; 3],
    ) -> &'static str {
        let valid = samples.iter().all(|sample| sample.frame_valid());
        if !valid || !sequences_advance(samples.map(|sample| sample.frame_sequence())) {
            return "raw_scaler_ordered_evidence_inconclusive";
        }
        if samples[1..]
            .iter()
            .all(|sample| sample.ordered_signature() == samples[0].ordered_signature())
        {
            "raw_scaler_ordered_stable"
        } else {
            // A scene transition can also change this passive fingerprint.
            // Root-cause attribution therefore requires independent proof that
            // the source framebuffer and scene remained byte-stable.
            "raw_scaler_order_changed_requires_static_source_proof"
        }
    }

    pub(super) fn scaler_fetch_ordered_classification(
        samples: [&mister_magik_video_diagnostics_contract::ScalerFetchOrderedSignatureState; 3],
    ) -> &'static str {
        let valid = samples.iter().all(|sample| sample.capture_valid());
        let fault_free = samples.iter().all(|sample| sample.fault_flags() == 0);
        if !valid
            || !fault_free
            || !sequences_advance(samples.map(|sample| sample.capture_sequence()))
        {
            return "scaler_fetch_ordered_evidence_inconclusive";
        }
        if samples[1..]
            .iter()
            .all(|sample| sample.ordered_signature() == samples[0].ordered_signature())
        {
            "scaler_fetch_ordered_stable"
        } else {
            // A source or scene transition can also change this passive fetch
            // fingerprint, so attribution requires independent byte-stability.
            "scaler_fetch_order_changed_requires_static_source_proof"
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(super) struct PathActivityFlags {
        pub(super) final_path: u16,
        pub(super) scaler_raw: u16,
        pub(super) post_osd: u16,
        pub(super) avalon: u16,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) struct HdmiOutputActivityDeltas {
        pub(super) no_de: u8,
        pub(super) de_all_zero: u8,
        pub(super) de_has_nonzero: u8,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum HdmiEvidenceProbeAction {
        UseNew,
        RetryNew,
        ReadLegacy,
        Fail,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum HdmiEvidenceAck {
        Present,
        Unsupported,
        Invalid,
    }

    pub(super) fn classify_hdmi_evidence_ack(magic_hi: u16, magic_lo: u16) -> HdmiEvidenceAck {
        classify_diagnostic_ack(
            magic_hi,
            magic_lo,
            mister_magik_video_diagnostics_contract::HDMI_EVIDENCE_MAGIC,
        )
    }

    pub(super) fn classify_hdmi_output_activity_ack(
        magic_hi: u16,
        magic_lo: u16,
    ) -> HdmiEvidenceAck {
        classify_diagnostic_ack(
            magic_hi,
            magic_lo,
            mister_magik_video_diagnostics_contract::HDMI_OUTPUT_ACTIVITY_MAGIC,
        )
    }

    fn classify_diagnostic_ack(
        magic_hi: u16,
        magic_lo: u16,
        expected_magic: u16,
    ) -> HdmiEvidenceAck {
        if magic_hi == expected_magic || magic_lo == expected_magic {
            HdmiEvidenceAck::Present
        } else if magic_hi == 0 && magic_lo == 0 {
            HdmiEvidenceAck::Unsupported
        } else {
            HdmiEvidenceAck::Invalid
        }
    }

    pub(super) fn hdmi_evidence_probe_action(
        error_kind: Option<io::ErrorKind>,
        retry_attempted: bool,
    ) -> HdmiEvidenceProbeAction {
        match (error_kind, retry_attempted) {
            (None, _) => HdmiEvidenceProbeAction::UseNew,
            (Some(io::ErrorKind::NotFound), false) => HdmiEvidenceProbeAction::ReadLegacy,
            (Some(io::ErrorKind::InvalidData | io::ErrorKind::TimedOut), false) => {
                HdmiEvidenceProbeAction::RetryNew
            }
            _ => HdmiEvidenceProbeAction::Fail,
        }
    }

    pub(super) fn video_diagnostics_ownership_state(
        owner_epoch_before: Option<u64>,
        owner_epoch_after: Option<u64>,
        latch_ownership_stable: Option<bool>,
        launcher_state_stable: bool,
    ) -> (bool, bool) {
        let changed = !launcher_state_stable
            || (owner_epoch_before.is_some() && owner_epoch_before != owner_epoch_after)
            || latch_ownership_stable == Some(false);
        let unverified = owner_epoch_before.is_none()
            || owner_epoch_after.is_none()
            || latch_ownership_stable.is_none();
        (changed, unverified)
    }

    pub(super) fn video_diagnostics_trigger_provenance(
        control: u16,
        avalon: u16,
        output: u16,
    ) -> (bool, bool, bool) {
        use mister_magik_video_diagnostics_contract as contract;
        (
            contract::control_trigger_has_valid_provenance(control),
            contract::avalon_trigger_has_valid_provenance(avalon),
            contract::output_trigger_has_valid_provenance(output),
        )
    }

    pub(super) fn hdmi_lock_classification(flags: u16) -> &'static str {
        use mister_magik_video_diagnostics_contract as contract;
        if flags & contract::HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST != 0 {
            "hdmi_pll_lock_lost"
        } else if flags & contract::HDMI_EVIDENCE_FLAG_LOCK_ARMED != 0
            && flags & contract::HDMI_EVIDENCE_FLAG_LOCK_CURRENT != 0
        {
            "hdmi_pll_locked"
        } else if flags & contract::HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH != 0 {
            "hdmi_pll_not_stably_armed"
        } else {
            "hdmi_pll_not_seen"
        }
    }

    pub(super) fn hdmi_output_activity_deltas(
        first_no_de: u8,
        first_de_all_zero: u8,
        first_de_has_nonzero: u8,
        second_no_de: u8,
        second_de_all_zero: u8,
        second_de_has_nonzero: u8,
    ) -> HdmiOutputActivityDeltas {
        let mask = mister_magik_video_diagnostics_contract::HDMI_OUTPUT_ACTIVITY_COUNTER_MASK as u8;
        HdmiOutputActivityDeltas {
            no_de: second_no_de.wrapping_sub(first_no_de) & mask,
            de_all_zero: second_de_all_zero.wrapping_sub(first_de_all_zero) & mask,
            de_has_nonzero: second_de_has_nonzero.wrapping_sub(first_de_has_nonzero) & mask,
        }
    }

    pub(super) fn hdmi_output_activity_classification(
        flags: u16,
        deltas: HdmiOutputActivityDeltas,
    ) -> &'static str {
        use mister_magik_video_diagnostics_contract as contract;
        if flags & contract::HDMI_OUTPUT_ACTIVITY_FLAG_COUNTER_COLLISION != 0 {
            "final_output_activity_invalid"
        } else {
            match (
                deltas.no_de != 0,
                deltas.de_all_zero != 0,
                deltas.de_has_nonzero != 0,
            ) {
                (false, false, false) => "final_output_no_completed_frame",
                (true, false, false) => "final_output_no_de",
                (false, true, false) => "final_output_de_all_zero",
                (false, false, true) => "final_output_de_has_nonzero",
                _ => "final_output_mixed",
            }
        }
    }

    fn path_counter_delta(first: u8, second: u8) -> u8 {
        let mask = mister_magik_video_diagnostics_contract::HDMI_PATH_ACTIVITY_COUNTER_MASK as u8;
        second.wrapping_sub(first) & mask
    }

    fn three_class_deltas(
        first_no_de: u8,
        first_zero: u8,
        first_nonzero: u8,
        second_no_de: u8,
        second_zero: u8,
        second_nonzero: u8,
    ) -> ThreeClassDeltas {
        ThreeClassDeltas {
            no_de: path_counter_delta(first_no_de, second_no_de),
            all_zero: path_counter_delta(first_zero, second_zero),
            nonzero: path_counter_delta(first_nonzero, second_nonzero),
        }
    }

    fn final_path_deltas(
        first: &mister_magik_video_diagnostics_contract::HdmiFinalPathActivity,
        second: &mister_magik_video_diagnostics_contract::HdmiFinalPathActivity,
    ) -> FinalPathDeltas {
        FinalPathDeltas {
            no_de: path_counter_delta(first.no_de_count(), second.no_de_count()),
            black_direct: path_counter_delta(
                first.black_direct_count(),
                second.black_direct_count(),
            ),
            black_scaled: path_counter_delta(
                first.black_scaled_count(),
                second.black_scaled_count(),
            ),
            black_mixed: path_counter_delta(first.black_mixed_count(), second.black_mixed_count()),
            nonzero: path_counter_delta(
                first.de_has_nonzero_count(),
                second.de_has_nonzero_count(),
            ),
        }
    }

    fn avalon_liveness_deltas(
        first: &mister_magik_video_diagnostics_contract::HdmiAvalonLivenessActivity,
        second: &mister_magik_video_diagnostics_contract::HdmiAvalonLivenessActivity,
    ) -> AvalonLivenessDeltas {
        AvalonLivenessDeltas {
            bucket: path_counter_delta(first.bucket_count(), second.bucket_count()),
            request: path_counter_delta(first.request_count(), second.request_count()),
            accepted: path_counter_delta(first.accepted_count(), second.accepted_count()),
            returned: path_counter_delta(first.returned_count(), second.returned_count()),
        }
    }

    fn scaler_fetch_deltas(
        first: &mister_magik_video_diagnostics_contract::HdmiScalerFetchActivity,
        second: &mister_magik_video_diagnostics_contract::HdmiScalerFetchActivity,
    ) -> ScalerFetchDeltas {
        use mister_magik_video_diagnostics_contract as contract;

        ScalerFetchDeltas {
            batch_two: second
                .batch_two_count()
                .wrapping_sub(first.batch_two_count())
                & contract::HDMI_SCALER_FETCH_ACTIVITY_BATCH_TWO_COUNT_MASK as u8,
            starved_frame: second
                .starved_frame_count()
                .wrapping_sub(first.starved_frame_count())
                & contract::HDMI_SCALER_FETCH_ACTIVITY_STARVED_FRAME_COUNT_MASK as u8,
        }
    }

    pub(super) fn scaler_fetch_classification(
        flags: u16,
        deltas: ScalerFetchDeltas,
    ) -> &'static str {
        use mister_magik_video_diagnostics_contract as contract;
        if flags
            & (contract::HDMI_SCALER_FETCH_ACTIVITY_FLAG_COMPLETION_DELTA_INVALID
                | contract::HDMI_SCALER_FETCH_ACTIVITY_FLAG_COMPLETION_LEVEL_INVALID)
            != 0
        {
            "scaler_fetch_evidence_invalid"
        } else if flags & contract::HDMI_SCALER_FETCH_ACTIVITY_FLAG_SNAPSHOT_VALID == 0 {
            "scaler_fetch_snapshot_unavailable"
        } else if deltas.starved_frame != 0 {
            "scaler_fetch_stalled_with_two_reads"
        } else if deltas.batch_two != 0 {
            "scaler_fetch_recovered_two_completion_batch"
        } else {
            "scaler_fetch_no_fault_observed"
        }
    }

    pub(super) fn detailed_path_classification(
        flags: PathActivityFlags,
        final_deltas: FinalPathDeltas,
        raw_deltas: ThreeClassDeltas,
        post_deltas: ThreeClassDeltas,
        avalon_deltas: AvalonLivenessDeltas,
    ) -> &'static str {
        use mister_magik_video_diagnostics_contract as contract;
        if flags.final_path & contract::HDMI_FINAL_PATH_ACTIVITY_FLAG_COUNTER_COLLISION != 0
            || flags.scaler_raw & contract::HDMI_SCALER_RAW_ACTIVITY_FLAG_COUNTER_COLLISION != 0
            || flags.post_osd & contract::HDMI_POST_OSD_ACTIVITY_FLAG_COUNTER_COLLISION != 0
            || final_deltas.black_mixed != 0
        {
            return "video_path_evidence_invalid";
        }
        let final_classes = [
            final_deltas.no_de,
            final_deltas.black_direct,
            final_deltas.black_scaled,
            final_deltas.nonzero,
        ];
        if final_classes.iter().filter(|delta| **delta != 0).count() != 1
            || final_classes.iter().copied().max().unwrap_or(0) < 2
        {
            return "video_path_inconclusive";
        }
        if final_deltas.nonzero >= 2 {
            return "final_output_de_has_nonzero";
        }
        if final_deltas.no_de >= 2 {
            return "final_output_no_de";
        }
        if final_deltas.black_direct >= 2 {
            return "final_output_black_direct_path";
        }

        let raw_classes = [raw_deltas.no_de, raw_deltas.all_zero, raw_deltas.nonzero];
        let post_classes = [post_deltas.no_de, post_deltas.all_zero, post_deltas.nonzero];
        if raw_classes.iter().filter(|delta| **delta != 0).count() != 1
            || post_classes.iter().filter(|delta| **delta != 0).count() != 1
            || raw_classes.iter().copied().max().unwrap_or(0) < 2
            || post_classes.iter().copied().max().unwrap_or(0) < 2
        {
            return "final_output_black_scaled_path_inconclusive";
        }
        if raw_deltas.nonzero != 0 && post_deltas.all_zero != 0 {
            return "post_scaler_processing_blacked_video";
        }
        if post_deltas.nonzero != 0 {
            return "final_output_staging_blacked_video";
        }
        if raw_deltas.no_de != 0 {
            return "scaler_raster_missing_de";
        }
        if raw_deltas.all_zero == 0 {
            return "final_output_black_scaled_path_inconclusive";
        }
        if flags.avalon & contract::HDMI_AVALON_LIVENESS_ACTIVITY_FLAG_BUCKET_VALID == 0 {
            return "scaler_raw_black_avalon_unavailable";
        }
        if avalon_deltas.bucket == 0 {
            return "scaler_raw_black_no_observed_avalon_bucket";
        }
        match (
            avalon_deltas.request != 0,
            avalon_deltas.accepted != 0,
            avalon_deltas.returned != 0,
        ) {
            (false, false, false) => "scaler_raw_black_no_framebuffer_requests",
            (true, false, false) => "scaler_raw_black_framebuffer_requests_blocked",
            (_, true, false) => "scaler_raw_black_accepted_without_observed_returns",
            (_, _, true) => "scaler_raw_black_with_framebuffer_returns",
        }
    }

    impl HdmiPathActivityWindow {
        fn to_json(&self) -> Value {
            use mister_magik_video_diagnostics_contract as contract;
            let final_flags = self.final_first.flags() | self.final_second.flags();
            let raw_flags = self.raw_first.flags() | self.raw_second.flags();
            let post_flags = self.post_first.flags() | self.post_second.flags();
            let avalon_flags = self.avalon_first.flags() | self.avalon_second.flags();
            let final_deltas = final_path_deltas(&self.final_first, &self.final_second);
            let raw_deltas = three_class_deltas(
                self.raw_first.no_de_count(),
                self.raw_first.de_all_zero_count(),
                self.raw_first.de_has_nonzero_count(),
                self.raw_second.no_de_count(),
                self.raw_second.de_all_zero_count(),
                self.raw_second.de_has_nonzero_count(),
            );
            let post_deltas = three_class_deltas(
                self.post_first.no_de_count(),
                self.post_first.de_all_zero_count(),
                self.post_first.de_has_nonzero_count(),
                self.post_second.no_de_count(),
                self.post_second.de_all_zero_count(),
                self.post_second.de_has_nonzero_count(),
            );
            let avalon_deltas = avalon_liveness_deltas(&self.avalon_first, &self.avalon_second);
            let scaler_fetch = match (&self.scaler_fetch_first, &self.scaler_fetch_second) {
                (Some(first), Some(second)) => {
                    let flags = first.flags() | second.flags();
                    let deltas = scaler_fetch_deltas(first, second);
                    json!({
                        "classification": scaler_fetch_classification(flags, deltas),
                        "snapshot_valid": flags
                            & contract::HDMI_SCALER_FETCH_ACTIVITY_FLAG_SNAPSHOT_VALID != 0,
                        "completion_delta_invalid": flags
                            & contract::HDMI_SCALER_FETCH_ACTIVITY_FLAG_COMPLETION_DELTA_INVALID != 0,
                        "completion_level_invalid": flags
                            & contract::HDMI_SCALER_FETCH_ACTIVITY_FLAG_COMPLETION_LEVEL_INVALID != 0,
                        "state_retired": true,
                        "deltas": {
                            "batch_two": deltas.batch_two,
                            "starved_frame": deltas.starved_frame,
                        },
                        "first_raw_words": first.words.as_slice(),
                        "second_raw_words": second.words.as_slice(),
                    })
                }
                _ => Value::Null,
            };
            let classification = if self.sample_interval_us < 80_000 {
                detailed_path_classification(
                    PathActivityFlags {
                        final_path: final_flags,
                        scaler_raw: raw_flags,
                        post_osd: post_flags,
                        avalon: avalon_flags,
                    },
                    final_deltas,
                    raw_deltas,
                    post_deltas,
                    avalon_deltas,
                )
            } else {
                "video_path_sample_window_too_long"
            };
            json!({
                "classification": classification,
                "sample_interval_us": self.sample_interval_us,
                "final": {
                    "frame_valid": final_flags
                        & contract::HDMI_FINAL_PATH_ACTIVITY_FLAG_FRAME_VALID != 0,
                    "counter_collision": final_flags
                        & contract::HDMI_FINAL_PATH_ACTIVITY_FLAG_COUNTER_COLLISION != 0,
                    "deltas": {
                        "no_de": final_deltas.no_de,
                        "black_direct": final_deltas.black_direct,
                        "black_scaled": final_deltas.black_scaled,
                        "black_mixed": final_deltas.black_mixed,
                        "nonzero": final_deltas.nonzero,
                    },
                    "first_raw_words": self.final_first.words.as_slice(),
                    "second_raw_words": self.final_second.words.as_slice(),
                },
                "scaler_raw": {
                    "frame_valid": raw_flags
                        & contract::HDMI_SCALER_RAW_ACTIVITY_FLAG_FRAME_VALID != 0,
                    "counter_collision": raw_flags
                        & contract::HDMI_SCALER_RAW_ACTIVITY_FLAG_COUNTER_COLLISION != 0,
                    "deltas": {
                        "no_de": raw_deltas.no_de,
                        "all_zero": raw_deltas.all_zero,
                        "nonzero": raw_deltas.nonzero,
                    },
                    "first_raw_words": self.raw_first.words.as_slice(),
                    "second_raw_words": self.raw_second.words.as_slice(),
                },
                "post_osd": {
                    "frame_valid": post_flags
                        & contract::HDMI_POST_OSD_ACTIVITY_FLAG_FRAME_VALID != 0,
                    "counter_collision": post_flags
                        & contract::HDMI_POST_OSD_ACTIVITY_FLAG_COUNTER_COLLISION != 0,
                    "deltas": {
                        "no_de": post_deltas.no_de,
                        "all_zero": post_deltas.all_zero,
                        "nonzero": post_deltas.nonzero,
                    },
                    "first_raw_words": self.post_first.words.as_slice(),
                    "second_raw_words": self.post_second.words.as_slice(),
                },
                "avalon": {
                    "bucket_valid": avalon_flags
                        & contract::HDMI_AVALON_LIVENESS_ACTIVITY_FLAG_BUCKET_VALID != 0,
                    "deltas": {
                        "bucket": avalon_deltas.bucket,
                        "request": avalon_deltas.request,
                        "accepted": avalon_deltas.accepted,
                        "returned": avalon_deltas.returned,
                    },
                    "first_raw_words": self.avalon_first.words.as_slice(),
                    "second_raw_words": self.avalon_second.words.as_slice(),
                },
                "scaler_fetch": scaler_fetch,
            })
        }
    }

    impl VideoDiagnosticsReadout {
        fn to_json(&self, context: VideoDiagnosticsJsonContext) -> Value {
            match self {
                Self::ScalerFetchLiveness(readout) => {
                    use mister_magik_video_diagnostics_contract as contract;
                    let samples = [
                        &readout.samples[0],
                        &readout.samples[1],
                        &readout.samples[2],
                    ];
                    let classification = scaler_fetch_liveness_classification(samples);
                    let valid_samples = samples.iter().all(|sample| sample.record_valid());
                    let advancing = nibble_sequences_advance(
                        samples.map(|sample| sample.publication_sequence()),
                    );
                    let classification_stable =
                        classification != "scaler_fetch_liveness_evidence_inconclusive";
                    let coherent = context.owner_stable
                        && context.latch_ownership_stable == Some(true)
                        && context.launcher_state_stable
                        && valid_samples
                        && advancing
                        && classification_stable;
                    json!({
                        "schema": "mister-magik-fpga-video-diagnostics-v2",
                        "diagnostic_architecture": contract::SCALER_FETCH_LIVENESS_STATE_ARCHITECTURE,
                        "available": true,
                        "coherent": coherent,
                        "classification": if coherent { classification } else { "scaler_fetch_liveness_evidence_inconclusive" },
                        "sink_visibility": "unobserved",
                        "capture_start_monotonic_us": context.capture_start_monotonic_us,
                        "capture_end_monotonic_us": context.capture_end_monotonic_us,
                        "owner_epoch_before": context.owner_epoch_before,
                        "owner_epoch_after": context.owner_epoch_after,
                        "latch_status": context.latch_status_json,
                        "coherence": {
                            "three_samples_valid": valid_samples,
                            "publication_sequence_advancing": advancing,
                            "classification_stable": classification_stable,
                            "sample_interval_us": readout.sample_interval_us,
                            "latch_ownership_stable": context.latch_ownership_stable,
                            "launcher_state_stable": context.launcher_state_stable,
                            "ownership_check_error": context.ownership_check_error,
                        },
                        "capabilities": {
                            "passive_video_observer": true,
                            "scaler_scheduler_state": false,
                            "scaler_pipeline_state": false,
                            "scaler_copy_retirement": false,
                            "scaler_fetch_liveness": true,
                            "scaler_fetch_ordered_signature": false,
                            "raw_scaler_ordered_signature": false,
                            "pixel_observer": false,
                            "pll_observer": false,
                        },
                        "scaler_fetch_liveness_state": {
                            "record_valid": samples.iter().map(|sample| sample.record_valid()).collect::<Vec<_>>(),
                            "normal_liveness_seen": samples.iter().map(|sample| sample.normal_liveness_seen()).collect::<Vec<_>>(),
                            "first_stall_valid": samples.iter().map(|sample| sample.first_stall_valid()).collect::<Vec<_>>(),
                            "observer_fault": samples.iter().map(|sample| sample.observer_fault()).collect::<Vec<_>>(),
                            "flags": samples.iter().map(|sample| sample.flags()).collect::<Vec<_>>(),
                            "publication_sequence": samples.iter().map(|sample| sample.publication_sequence()).collect::<Vec<_>>(),
                            "frozen_address_fold": samples.iter().map(|sample| sample.frozen_address_fold()).collect::<Vec<_>>(),
                            "return_phase": samples.iter().map(|sample| sample.return_phase()).collect::<Vec<_>>(),
                            "fifo_depth": samples.iter().map(|sample| sample.fifo_depth()).collect::<Vec<_>>(),
                            "monitor_state": samples.iter().map(|sample| sample.monitor_state()).collect::<Vec<_>>(),
                            "frozen_cause": samples.iter().map(|sample| sample.frozen_cause()).collect::<Vec<_>>(),
                            "frozen_return_phase": samples.iter().map(|sample| sample.frozen_return_phase()).collect::<Vec<_>>(),
                            "frozen_fifo_depth": samples.iter().map(|sample| sample.frozen_fifo_depth()).collect::<Vec<_>>(),
                            "raw_samples": readout.samples.iter().map(|sample| sample.words).collect::<Vec<_>>(),
                        },
                    })
                }
                Self::RawScaler(readout) => {
                    use mister_magik_video_diagnostics_contract::RawScalerState;
                    match &readout.samples {
                        [
                            RawScalerState::RawScalerOrderedSignatureV3(first),
                            RawScalerState::RawScalerOrderedSignatureV3(second),
                            RawScalerState::RawScalerOrderedSignatureV3(third),
                        ] => {
                            let samples = [first, second, third];
                            let valid_samples = samples.iter().all(|sample| sample.frame_valid());
                            let classification = raw_scaler_classification(samples);
                            let classification_stable =
                                classification != "raw_scaler_ordered_evidence_inconclusive";
                            let coherent = context.owner_stable
                                && context.latch_ownership_stable == Some(true)
                                && context.launcher_state_stable
                                && valid_samples
                                && classification_stable;
                            json!({
                                "schema": "mister-magik-fpga-video-diagnostics-v2",
                                "diagnostic_architecture": "raw-scaler-ordered-signature-v3",
                                "available": true,
                                "coherent": coherent,
                                "classification": if coherent { classification } else { "raw_scaler_ordered_evidence_inconclusive" },
                                "sink_visibility": "unobserved",
                                "capture_start_monotonic_us": context.capture_start_monotonic_us,
                                "capture_end_monotonic_us": context.capture_end_monotonic_us,
                                "owner_epoch_before": context.owner_epoch_before,
                                "owner_epoch_after": context.owner_epoch_after,
                                "latch_status": context.latch_status_json,
                                "coherence": {
                                    "three_samples_valid": valid_samples,
                                    "classification_stable": classification_stable,
                                    "sample_interval_us": readout.sample_interval_us,
                                    "latch_ownership_stable": context.latch_ownership_stable,
                                    "launcher_state_stable": context.launcher_state_stable,
                                    "ownership_check_error": context.ownership_check_error,
                                },
                                "capabilities": {
                                    "passive_video_observer": true,
                                    "scaler_scheduler_state": false,
                                    "scaler_pipeline_state": false,
                                    "scaler_copy_retirement": false,
                                    "scaler_fetch_ordered_signature": false,
                                    "raw_scaler_ordered_signature": true,
                                    "pixel_observer": true,
                                    "pll_observer": false,
                                },
                                "raw_scaler_state": {
                                    "frame_valid": samples.iter().map(|sample| sample.frame_valid()).collect::<Vec<_>>(),
                                    "flags": samples.iter().map(|sample| sample.flags()).collect::<Vec<_>>(),
                                    "frame_sequence": samples.iter().map(|sample| sample.frame_sequence()).collect::<Vec<_>>(),
                                    "ordered_signature": samples.iter().map(|sample| format!("{:04x}", sample.ordered_signature())).collect::<Vec<_>>(),
                                    "raw_samples": readout.samples.iter().map(|sample| *sample.words()).collect::<Vec<_>>(),
                                },
                            })
                        }
                        [
                            RawScalerState::ScalerFetchOrderedSignatureV1(first),
                            RawScalerState::ScalerFetchOrderedSignatureV1(second),
                            RawScalerState::ScalerFetchOrderedSignatureV1(third),
                        ] => {
                            let samples = [first, second, third];
                            let valid_samples = samples.iter().all(|sample| sample.capture_valid());
                            let classification = scaler_fetch_ordered_classification(samples);
                            let classification_stable =
                                classification != "scaler_fetch_ordered_evidence_inconclusive";
                            let coherent = context.owner_stable
                                && context.latch_ownership_stable == Some(true)
                                && context.launcher_state_stable
                                && valid_samples
                                && classification_stable;
                            json!({
                                "schema": "mister-magik-fpga-video-diagnostics-v2",
                                "diagnostic_architecture": "scaler-fetch-ordered-signature-v1",
                                "available": true,
                                "coherent": coherent,
                                "classification": if coherent { classification } else { "scaler_fetch_ordered_evidence_inconclusive" },
                                "sink_visibility": "unobserved",
                                "capture_start_monotonic_us": context.capture_start_monotonic_us,
                                "capture_end_monotonic_us": context.capture_end_monotonic_us,
                                "owner_epoch_before": context.owner_epoch_before,
                                "owner_epoch_after": context.owner_epoch_after,
                                "latch_status": context.latch_status_json,
                                "coherence": {
                                    "three_samples_valid": valid_samples,
                                    "classification_stable": classification_stable,
                                    "sample_interval_us": readout.sample_interval_us,
                                    "latch_ownership_stable": context.latch_ownership_stable,
                                    "launcher_state_stable": context.launcher_state_stable,
                                    "ownership_check_error": context.ownership_check_error,
                                },
                                "capabilities": {
                                    "passive_video_observer": true,
                                    "scaler_scheduler_state": false,
                                    "scaler_pipeline_state": false,
                                    "scaler_copy_retirement": false,
                                    "scaler_fetch_ordered_signature": true,
                                    "raw_scaler_ordered_signature": false,
                                    "pixel_observer": false,
                                    "pll_observer": false,
                                },
                                "scaler_fetch_state": {
                                    "capture_valid": samples.iter().map(|sample| sample.capture_valid()).collect::<Vec<_>>(),
                                    "flags": samples.iter().map(|sample| sample.flags()).collect::<Vec<_>>(),
                                    "fault_flags": samples.iter().map(|sample| sample.fault_flags()).collect::<Vec<_>>(),
                                    "capture_sequence": samples.iter().map(|sample| sample.capture_sequence()).collect::<Vec<_>>(),
                                    "ordered_signature": samples.iter().map(|sample| format!("{:04x}", sample.ordered_signature())).collect::<Vec<_>>(),
                                    "raw_samples": readout.samples.iter().map(|sample| *sample.words()).collect::<Vec<_>>(),
                                },
                            })
                        }
                        _ => json!({
                            "schema": "mister-magik-fpga-video-diagnostics-v2",
                            "diagnostic_architecture": "raw-scaler-schema-transition",
                            "available": false,
                            "coherent": false,
                            "classification": "scaler_fetch_ordered_evidence_inconclusive",
                            "sink_visibility": "unobserved",
                            "reason": "raw scaler schema changed during bounded sampling",
                        }),
                    }
                }
                Self::HdmiLock(readout) => {
                    use mister_magik_video_diagnostics_contract as contract;
                    let evidence = &readout.lock;
                    let flags = evidence.flags();
                    let coherent = context.owner_stable
                        && context.latch_ownership_stable == Some(true)
                        && context.launcher_state_stable;
                    let output_activity = readout.output_activity.as_ref().map(|window| {
                        let first_flags = window.first.flags();
                        let second_flags = window.second.flags();
                        let deltas = hdmi_output_activity_deltas(
                            window.first.no_de_count(),
                            window.first.de_all_zero_count(),
                            window.first.de_has_nonzero_count(),
                            window.second.no_de_count(),
                            window.second.de_all_zero_count(),
                            window.second.de_has_nonzero_count(),
                        );
                        json!({
                            "classification": hdmi_output_activity_classification(
                                first_flags | second_flags,
                                deltas,
                            ),
                            "sample_interval_us": window.sample_interval_us,
                            "counter_collision": (first_flags | second_flags)
                                & contract::HDMI_OUTPUT_ACTIVITY_FLAG_COUNTER_COLLISION
                                != 0,
                            "first": {
                                "frame_valid": first_flags
                                    & contract::HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID
                                    != 0,
                                "no_de_count": window.first.no_de_count(),
                                "de_all_zero_count": window.first.de_all_zero_count(),
                                "de_has_nonzero_count": window.first.de_has_nonzero_count(),
                                "raw_words": window.first.words.as_slice(),
                            },
                            "second": {
                                "frame_valid": second_flags
                                    & contract::HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID
                                    != 0,
                                "no_de_count": window.second.no_de_count(),
                                "de_all_zero_count": window.second.de_all_zero_count(),
                                "de_has_nonzero_count": window.second.de_has_nonzero_count(),
                                "raw_words": window.second.words.as_slice(),
                            },
                            "deltas": {
                                "no_de": deltas.no_de,
                                "de_all_zero": deltas.de_all_zero,
                                "de_has_nonzero": deltas.de_has_nonzero,
                            },
                        })
                    });
                    let path_activity = readout
                        .path_activity
                        .as_ref()
                        .map(HdmiPathActivityWindow::to_json);
                    let classification = path_activity
                        .as_ref()
                        .and_then(|value| value.get("classification"))
                        .and_then(Value::as_str)
                        .or_else(|| {
                            output_activity
                                .as_ref()
                                .and_then(|value| value.get("classification"))
                                .and_then(Value::as_str)
                        })
                        .unwrap_or_else(|| hdmi_lock_classification(flags));
                    json!({
                        "schema": "mister-magik-fpga-video-diagnostics-v2",
                        "diagnostic_architecture": "video-path-evidence-v1",
                        "available": true,
                        "coherent": coherent,
                        "classification": classification,
                        "capture_start_monotonic_us": context.capture_start_monotonic_us,
                        "capture_end_monotonic_us": context.capture_end_monotonic_us,
                        "owner_epoch_before": context.owner_epoch_before,
                        "owner_epoch_after": context.owner_epoch_after,
                        "latch_status": context.latch_status_json,
                        "coherence": {
                            "latch_ownership_stable": context.latch_ownership_stable,
                            "launcher_state_stable": context.launcher_state_stable,
                            "ownership_check_error": context.ownership_check_error,
                        },
                        "capabilities": {
                            "physical_hdmi_pll_lock": true,
                            "final_hdmi_output": output_activity.is_some(),
                            "final_mux_provenance": path_activity.is_some(),
                            "scaler_raw_output": path_activity.is_some(),
                            "post_osd_output": path_activity.is_some(),
                            "avalon_read_path": path_activity.is_some(),
                        },
                        "hdmi_lock": {
                            "seen_high": flags & contract::HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH != 0,
                            "armed": flags & contract::HDMI_EVIDENCE_FLAG_LOCK_ARMED != 0,
                            "current": flags & contract::HDMI_EVIDENCE_FLAG_LOCK_CURRENT != 0,
                            "ever_lost": flags & contract::HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST != 0,
                            "loss_count": evidence.words[contract::HDMI_EVIDENCE_LOCK_LOSS_COUNT_WORD],
                            "loss_count_overflow": flags
                                & contract::HDMI_EVIDENCE_FLAG_LOCK_LOSS_COUNT_OVERFLOW
                                != 0,
                            "raw_words": evidence.words.as_slice(),
                        },
                        "final_hdmi_output_activity": output_activity,
                        "video_path_activity": path_activity,
                    })
                }
                Self::Legacy(readout) => readout.to_json(context),
            }
        }
    }

    impl LegacyVideoDiagnosticsReadout {
        fn generations_match(&self) -> bool {
            self.control_before.generation == self.control_after.generation
                && self.control_after.generation == self.avalon.generation
                && self.control_after.generation == self.output.generation
        }

        fn to_json(&self, context: VideoDiagnosticsJsonContext) -> Value {
            let VideoDiagnosticsJsonContext {
                owner_stable,
                latch_ownership_stable,
                launcher_state_stable,
                ownership_check_error,
                owner_epoch_before,
                owner_epoch_after,
                latch_status_json,
                capture_start_monotonic_us,
                capture_end_monotonic_us,
            } = context;
            use mister_magik_video_diagnostics_contract as contract;

            let control_generation_stable =
                self.control_before.generation == self.control_after.generation;
            let generations_match = self.generations_match();
            let control_route_epoch =
                self.control_after.words[contract::VIDEO_DIAGNOSTICS_CONTROL_ROUTE_EPOCH];
            let avalon_route_epoch =
                self.avalon.words[contract::VIDEO_DIAGNOSTICS_AVALON_ROUTE_EPOCH];
            let output_route_epoch =
                self.output.words[contract::VIDEO_DIAGNOSTICS_OUTPUT_ROUTE_EPOCH];
            let route_epochs_match = control_route_epoch == avalon_route_epoch
                && control_route_epoch == output_route_epoch;
            let missing_domains =
                self.control_after.words[contract::VIDEO_DIAGNOSTICS_CONTROL_MISSING_DOMAINS];
            let all_frozen =
                matches!(
                    self.control_after.state,
                    contract::VideoDiagnosticsState::Frozen
                ) && matches!(self.avalon.state, contract::VideoDiagnosticsState::Frozen)
                    && matches!(self.output.state, contract::VideoDiagnosticsState::Frozen);
            let mailbox_overrun = self.control_after.words[1]
                & contract::VIDEO_DIAGNOSTICS_STATE_FLAGS_MAILBOX_OVERRUN
                != 0
                || self.avalon.words[1] & contract::VIDEO_DIAGNOSTICS_STATE_FLAGS_MAILBOX_OVERRUN
                    != 0
                || self.output.words[1] & contract::VIDEO_DIAGNOSTICS_STATE_FLAGS_MAILBOX_OVERRUN
                    != 0;
            let (ownership_changed, ownership_unverified) = video_diagnostics_ownership_state(
                owner_epoch_before,
                owner_epoch_after,
                latch_ownership_stable,
                launcher_state_stable,
            );
            let (
                control_trigger_provenance_valid,
                avalon_trigger_provenance_valid,
                output_trigger_provenance_valid,
            ) = video_diagnostics_trigger_provenance(
                self.control_after.trigger,
                self.avalon.trigger,
                self.output.trigger,
            );
            let trigger_provenance_valid = control_trigger_provenance_valid
                && avalon_trigger_provenance_valid
                && output_trigger_provenance_valid;
            let coherent = owner_stable
                && latch_ownership_stable == Some(true)
                && generations_match
                && route_epochs_match
                && missing_domains == 0
                && all_frozen
                && !mailbox_overrun
                && trigger_provenance_valid;
            let partial = missing_domains != 0
                || mailbox_overrun
                || ownership_unverified
                || matches!(
                    self.control_after.state,
                    contract::VideoDiagnosticsState::Partial
                );
            let trigger = self.control_after.trigger;
            let classification = if partial {
                "partial"
            } else if ownership_changed {
                "control_or_clock"
            } else if !trigger_provenance_valid {
                "incoherent_trigger_provenance"
            } else if !coherent {
                "unclassified"
            } else {
                match trigger {
                    contract::VIDEO_DIAGNOSTICS_TRIGGER_LEGACY_OWNED
                    | contract::VIDEO_DIAGNOSTICS_TRIGGER_ROUTE_DIVERGENCE
                    | contract::VIDEO_DIAGNOSTICS_TRIGGER_OWNED_OSD_WRITE => "legacy_control",
                    contract::VIDEO_DIAGNOSTICS_TRIGGER_CONTROL_OR_CLOCK => "control_or_clock",
                    contract::VIDEO_DIAGNOSTICS_TRIGGER_AVALON_NO_READS => "avalon_no_reads",
                    contract::VIDEO_DIAGNOSTICS_TRIGGER_AVALON_ADDRESS
                    | contract::VIDEO_DIAGNOSTICS_TRIGGER_AVALON_BURST
                    | contract::VIDEO_DIAGNOSTICS_TRIGGER_AVALON_RETURN
                    | contract::VIDEO_DIAGNOSTICS_TRIGGER_AVALON_TIMEOUT => {
                        "avalon_stall_or_return"
                    }
                    contract::VIDEO_DIAGNOSTICS_TRIGGER_FINAL_BLACK => "final_black",
                    contract::VIDEO_DIAGNOSTICS_TRIGGER_FINAL_WHITE => "final_white",
                    contract::VIDEO_DIAGNOSTICS_TRIGGER_FINAL_TIMING => "final_timing",
                    _ => "unclassified",
                }
            };
            json!({
                "schema": "mister-magik-fpga-video-diagnostics-v1",
                "available": true,
                "coherent": coherent,
                "classification": classification,
                "capture_start_monotonic_us": capture_start_monotonic_us,
                "capture_end_monotonic_us": capture_end_monotonic_us,
                "owner_epoch_before": owner_epoch_before,
                "owner_epoch_after": owner_epoch_after,
                "latch_status": latch_status_json,
                "coherence": {
                    "control_generation_stable": control_generation_stable,
                    "generations_match": generations_match,
                    "route_epochs_match": route_epochs_match,
                    "missing_domains": missing_domains,
                    "all_domains_frozen": all_frozen,
                    "mailbox_overrun": mailbox_overrun,
                    "latch_ownership_stable": latch_ownership_stable,
                    "launcher_state_stable": launcher_state_stable,
                    "ownership_check_error": ownership_check_error,
                    "trigger_provenance_valid": trigger_provenance_valid,
                },
                "control": {
                    "state": video_diagnostics_state_name(self.control_after.state),
                    "trigger": self.control_after.trigger,
                    "trigger_provenance_valid": control_trigger_provenance_valid,
                    "generation": self.control_after.generation,
                    "route_epoch": control_route_epoch,
                    "state_flags": self.control_after.words[contract::VIDEO_DIAGNOSTICS_CONTROL_STATE_FLAGS],
                    "route_flags": self.control_after.words
                        [contract::VIDEO_DIAGNOSTICS_CONTROL_ROUTE_CONTROL_FLAGS]
                        & contract::VIDEO_DIAGNOSTICS_ROUTE_FLAGS_MASK,
                    "control_fault_flags": self.control_after.words
                        [contract::VIDEO_DIAGNOSTICS_CONTROL_ROUTE_CONTROL_FLAGS]
                        >> 8,
                    "legacy_disposition": self.control_after.words
                        [contract::VIDEO_DIAGNOSTICS_CONTROL_LEGACY_MASK_DISPOSITION]
                        >> 12,
                    "legacy_word_mask": self.control_after.words
                        [contract::VIDEO_DIAGNOSTICS_CONTROL_LEGACY_MASK_DISPOSITION]
                        & 0x03ff,
                    "raw_words": self.control_after.words.as_slice(),
                },
                "avalon": {
                    "state": video_diagnostics_state_name(self.avalon.state),
                    "trigger": self.avalon.trigger,
                    "trigger_provenance_valid": avalon_trigger_provenance_valid,
                    "generation": self.avalon.generation,
                    "route_epoch": avalon_route_epoch,
                    "state_flags": self.avalon.words[contract::VIDEO_DIAGNOSTICS_AVALON_STATE_FLAGS],
                    "route_flags": self.avalon.words[contract::VIDEO_DIAGNOSTICS_AVALON_ROUTE_FLAGS],
                    "fault_flags": self.avalon.words[contract::VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS],
                    "accepted_bursts": self.avalon.words[contract::VIDEO_DIAGNOSTICS_AVALON_ACCEPTED_BURSTS],
                    "expected_beats": u32::from(self.avalon.words[contract::VIDEO_DIAGNOSTICS_AVALON_ACCEPTED_BURSTS]) * 128,
                    "returned_beats": self.avalon.words[contract::VIDEO_DIAGNOSTICS_AVALON_RETURNED_BEATS],
                    "raw_words": self.avalon.words.as_slice(),
                },
                "output": {
                    "state": video_diagnostics_state_name(self.output.state),
                    "trigger": self.output.trigger,
                    "trigger_provenance_valid": output_trigger_provenance_valid,
                    "generation": self.output.generation,
                    "route_epoch": output_route_epoch,
                    "state_flags": self.output.words[contract::VIDEO_DIAGNOSTICS_OUTPUT_STATE_FLAGS],
                    "source_flags": self.output.words[contract::VIDEO_DIAGNOSTICS_OUTPUT_SOURCE_FLAGS],
                    "control_flags": self.output.words[contract::VIDEO_DIAGNOSTICS_OUTPUT_CONTROL_FLAGS],
                    "fault_flags": self.output.words[contract::VIDEO_DIAGNOSTICS_OUTPUT_FAULT_SUMMARY] & 0x00ff,
                    "geometry_faults": (self.output.words[contract::VIDEO_DIAGNOSTICS_OUTPUT_FAULT_SUMMARY] >> 8) & 0x0007,
                    "raw_words": self.output.words.as_slice(),
                },
            })
        }
    }

    struct FpgaIo {
        base: *mut u8,
        _file: File,
        uio_lock: File,
        gpo: u32,
        latch_protocol: Option<mister_magik_latch_contract::LatchProtocol>,
        latch_capabilities: Option<mister_magik_latch_contract::LatchCapabilities>,
    }

    struct FpgaUioGuard {
        fd: std::os::fd::RawFd,
    }

    impl FpgaUioGuard {
        fn acquire(lock: &File, timeout: Duration) -> io::Result<Self> {
            let fd = lock.as_raw_fd();
            let started = Instant::now();
            loop {
                // SAFETY: fd is a valid open lock file descriptor and flock
                // does not access memory through the descriptor.
                if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                    return Ok(Self { fd });
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::WouldBlock {
                    return Err(error);
                }
                if started.elapsed() >= timeout {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "FPGA UIO transaction busy",
                    ));
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    impl Drop for FpgaUioGuard {
        fn drop(&mut self) {
            // SAFETY: fd belongs to the live FpgaIo lock file for longer than
            // this guard; LOCK_UN only releases this process's advisory flock.
            unsafe {
                libc::flock(self.fd, libc::LOCK_UN);
            }
        }
    }

    impl FpgaIo {
        fn open() -> io::Result<Self> {
            fs::create_dir_all("/tmp/mister-magik")?;
            let uio_lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(mister_magik_latch_contract::FPGA_UIO_LOCK_PATH)?;
            let file = OpenOptions::new().read(true).write(true).open("/dev/mem")?;
            let base = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    FPGA_MGR_LEN,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    file.as_raw_fd(),
                    FPGA_MGR_BASE as libc::off_t,
                )
            };
            if base == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                base: base.cast::<u8>(),
                _file: file,
                uio_lock,
                gpo: FPGA_BIT31,
                latch_protocol: None,
                latch_capabilities: None,
            })
        }

        fn lock_uio_transaction(&self) -> io::Result<FpgaUioGuard> {
            FpgaUioGuard::acquire(&self.uio_lock, Duration::from_millis(250))
        }

        fn read_latched_fbuf_status(&mut self) -> io::Result<LatchedFbufStatus> {
            let protocol = match self.latch_protocol {
                Some(protocol) => protocol,
                None => self.negotiate_latch_protocol()?,
            };
            match self.read_latched_fbuf_status_once(protocol) {
                Err(first)
                    if protocol == mister_magik_latch_contract::LatchProtocol::V5
                        && first.kind() == io::ErrorKind::InvalidData =>
                {
                    self.reset_spi_transport();
                    self.read_latched_fbuf_status_once(protocol)
                }
                result => result,
            }
        }

        fn read_presentation_telemetry(
            &mut self,
        ) -> io::Result<mister_magik_latch_contract::PresentationTelemetry> {
            let protocol = match self.latch_protocol {
                Some(protocol) => protocol,
                None => self.negotiate_latch_protocol()?,
            };
            if protocol != mister_magik_latch_contract::LatchProtocol::V5 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "authoritative presentation telemetry requires latch protocol v5",
                ));
            }
            match self.read_presentation_telemetry_once() {
                Err(first) if first.kind() == io::ErrorKind::InvalidData => {
                    self.reset_spi_transport();
                    self.read_presentation_telemetry_once()
                }
                result => result,
            }
        }

        fn read_video_diagnostics(&mut self) -> io::Result<VideoDiagnosticsReadout> {
            let scaler_fetch_liveness_probe_error =
                match self.read_scaler_fetch_liveness_diagnostics() {
                    Ok(readout) => {
                        return Ok(VideoDiagnosticsReadout::ScalerFetchLiveness(readout));
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => error.to_string(),
                    Err(error) => return Err(error),
                };
            match self.read_raw_scaler_diagnostics() {
                Ok(readout) => return Ok(VideoDiagnosticsReadout::RawScaler(readout)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            match self.read_hdmi_lock_evidence() {
                Ok(lock) => {
                    let output_activity = match self.read_hdmi_output_activity_window() {
                        Ok(window) => Some(window),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                        Err(error) => return Err(error),
                    };
                    let path_activity = match self.read_hdmi_path_activity_window() {
                        Ok(window) => Some(window),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                        Err(error) => return Err(error),
                    };
                    return Ok(VideoDiagnosticsReadout::HdmiLock(
                        HdmiLockDiagnosticsReadout {
                            lock,
                            output_activity,
                            path_activity,
                        },
                    ));
                }
                Err(error)
                    if hdmi_evidence_probe_action(Some(error.kind()), false)
                        == HdmiEvidenceProbeAction::ReadLegacy => {}
                Err(error) => return Err(error),
            }
            let legacy_result = match self.read_legacy_video_diagnostics_once() {
                Err(first) if first.kind() == io::ErrorKind::InvalidData => {
                    self.reset_spi_transport();
                    self.read_legacy_video_diagnostics_once()
                        .map(Box::new)
                        .map(VideoDiagnosticsReadout::Legacy)
                }
                Ok(first) if !first.generations_match() => {
                    self.reset_spi_transport();
                    self.read_legacy_video_diagnostics_once()
                        .map(Box::new)
                        .map(VideoDiagnosticsReadout::Legacy)
                }
                result => result.map(Box::new).map(VideoDiagnosticsReadout::Legacy),
            };
            legacy_result.map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "no passive observer responded; scaler_fetch_liveness_probe={scaler_fetch_liveness_probe_error}; final_probe={error}"
                        ),
                    )
                } else {
                    error
                }
            })
        }

        fn read_scaler_fetch_liveness_diagnostics(
            &mut self,
        ) -> io::Result<ScalerFetchLivenessDiagnosticsReadout> {
            let first = self.read_scaler_fetch_liveness_state()?;
            let first_started = Instant::now();
            thread::sleep(Duration::from_millis(25));
            let second = self.read_scaler_fetch_liveness_state().map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("scaler fetch liveness disappeared during sampling: {error}"),
                    )
                } else {
                    error
                }
            })?;
            let first_interval_us = first_started.elapsed().as_micros() as u64;
            let second_started = Instant::now();
            thread::sleep(Duration::from_millis(25));
            let third = self.read_scaler_fetch_liveness_state().map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("scaler fetch liveness disappeared during sampling: {error}"),
                    )
                } else {
                    error
                }
            })?;
            Ok(ScalerFetchLivenessDiagnosticsReadout {
                samples: [first, second, third],
                sample_interval_us: [
                    first_interval_us,
                    second_started.elapsed().as_micros() as u64,
                ],
            })
        }

        fn read_scaler_fetch_liveness_state(
            &mut self,
        ) -> io::Result<mister_magik_video_diagnostics_contract::ScalerFetchLivenessState> {
            use mister_magik_video_diagnostics_contract as contract;
            // Command 0x68 deliberately acknowledges only while a freshly
            // published bank is available. The short acknowledge/re-publish
            // handoff must not be mistaken for an unsupported architecture.
            let mut attempt = 0;
            let words = loop {
                match self.read_diagnostic_words::<{ contract::SCALER_FETCH_LIVENESS_STATE_WORDS }>(
                    contract::GET_SCALER_FETCH_LIVENESS_STATE,
                    contract::SCALER_FETCH_LIVENESS_STATE_MAGIC,
                    "scaler fetch liveness state",
                ) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound && attempt < 9 => {
                        attempt += 1;
                        thread::sleep(Duration::from_millis(1));
                    }
                    result => break result?,
                }
            };
            contract::decode_scaler_fetch_liveness_state(&words)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))
        }

        fn read_raw_scaler_diagnostics(&mut self) -> io::Result<RawScalerDiagnosticsReadout> {
            let first = self.read_raw_scaler_state()?;
            let first_started = Instant::now();
            thread::sleep(Duration::from_millis(25));
            let second = self.read_raw_scaler_state().map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("raw scaler state disappeared during sampling: {error}"),
                    )
                } else {
                    error
                }
            })?;
            let first_interval_us = first_started.elapsed().as_micros() as u64;
            let second_started = Instant::now();
            thread::sleep(Duration::from_millis(25));
            let third = self.read_raw_scaler_state().map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("raw scaler state disappeared during sampling: {error}"),
                    )
                } else {
                    error
                }
            })?;
            Ok(RawScalerDiagnosticsReadout {
                samples: [first, second, third],
                sample_interval_us: [
                    first_interval_us,
                    second_started.elapsed().as_micros() as u64,
                ],
            })
        }

        fn read_raw_scaler_state(
            &mut self,
        ) -> io::Result<mister_magik_video_diagnostics_contract::RawScalerState> {
            use mister_magik_video_diagnostics_contract as contract;
            let words = self.read_diagnostic_words::<{ contract::RAW_SCALER_STATE_WORDS }>(
                contract::GET_RAW_SCALER_STATE,
                contract::RAW_SCALER_STATE_MAGIC,
                "raw scaler state",
            )?;
            contract::decode_raw_scaler_state(&words)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))
        }

        fn read_hdmi_lock_evidence(
            &mut self,
        ) -> io::Result<mister_magik_video_diagnostics_contract::HdmiEvidence> {
            let first = self.read_hdmi_lock_evidence_once();
            match first {
                Err(first_error)
                    if hdmi_evidence_probe_action(Some(first_error.kind()), false)
                        == HdmiEvidenceProbeAction::RetryNew =>
                {
                    self.reset_spi_transport();
                    self.read_hdmi_lock_evidence_once().map_err(|retry_error| {
                        io::Error::new(
                            first_error.kind(),
                            format!(
                                "HDMI lock evidence failed after retry: first={first_error}; retry={retry_error}"
                            ),
                        )
                    })
                }
                result => result,
            }
        }

        fn read_hdmi_lock_evidence_once(
            &mut self,
        ) -> io::Result<mister_magik_video_diagnostics_contract::HdmiEvidence> {
            use mister_magik_video_diagnostics_contract as contract;
            let result = (|| {
                let (magic_hi, magic_lo) = self.cmd_capture(contract::GET_HDMI_EVIDENCE)?;
                match classify_hdmi_evidence_ack(magic_hi, magic_lo) {
                    HdmiEvidenceAck::Present => {}
                    HdmiEvidenceAck::Unsupported => {
                        return Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            "HDMI lock evidence command 0x60 is unsupported",
                        ));
                    }
                    HdmiEvidenceAck::Invalid => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "HDMI lock evidence acknowledgement is malformed: ack_high=0x{magic_hi:04x} ack_low=0x{magic_lo:04x}"
                            ),
                        ));
                    }
                }
                let mut words = [0u16; contract::HDMI_EVIDENCE_WORDS];
                for word in &mut words {
                    *word = self.spi_capture(0)?.1;
                }
                Ok(words)
            })();
            self.disable_io();
            let words = result?;
            contract::decode_hdmi_evidence(&words)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))
        }

        fn read_hdmi_output_activity_window(&mut self) -> io::Result<HdmiOutputActivityWindow> {
            let first = self.read_hdmi_output_activity()?;
            let sample_started = Instant::now();
            thread::sleep(Duration::from_millis(50));
            let second = self.read_hdmi_output_activity().map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "HDMI output activity became unsupported within one sample window: {error}"
                        ),
                    )
                } else {
                    error
                }
            })?;
            Ok(HdmiOutputActivityWindow {
                first,
                second,
                sample_interval_us: sample_started.elapsed().as_micros() as u64,
            })
        }

        fn read_hdmi_output_activity(
            &mut self,
        ) -> io::Result<mister_magik_video_diagnostics_contract::HdmiOutputActivity> {
            let first = self.read_hdmi_output_activity_once();
            match first {
                Err(first_error)
                    if hdmi_evidence_probe_action(Some(first_error.kind()), false)
                        == HdmiEvidenceProbeAction::RetryNew =>
                {
                    self.reset_spi_transport();
                    self.read_hdmi_output_activity_once()
                        .map_err(|retry_error| {
                            io::Error::new(
                                first_error.kind(),
                                format!(
                                    "HDMI output activity failed after retry: first={first_error}; retry={retry_error}"
                                ),
                            )
                        })
                }
                result => result,
            }
        }

        fn read_hdmi_output_activity_once(
            &mut self,
        ) -> io::Result<mister_magik_video_diagnostics_contract::HdmiOutputActivity> {
            use mister_magik_video_diagnostics_contract as contract;
            let result = (|| {
                let (magic_hi, magic_lo) = self.cmd_capture(contract::GET_HDMI_OUTPUT_ACTIVITY)?;
                match classify_hdmi_output_activity_ack(magic_hi, magic_lo) {
                    HdmiEvidenceAck::Present => {}
                    HdmiEvidenceAck::Unsupported => {
                        return Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            "HDMI output activity command 0x61 is unsupported",
                        ));
                    }
                    HdmiEvidenceAck::Invalid => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "HDMI output activity acknowledgement is malformed: ack_high=0x{magic_hi:04x} ack_low=0x{magic_lo:04x}"
                            ),
                        ));
                    }
                }
                let mut words = [0u16; contract::HDMI_OUTPUT_ACTIVITY_WORDS];
                for word in &mut words {
                    *word = self.spi_capture(0)?.1;
                }
                Ok(words)
            })();
            self.disable_io();
            let words = result?;
            contract::decode_hdmi_output_activity(&words)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))
        }

        fn read_diagnostic_words<const N: usize>(
            &mut self,
            command: u16,
            magic: u16,
            label: &str,
        ) -> io::Result<[u16; N]> {
            let first = self.read_diagnostic_words_once(command, magic, label);
            match first {
                Err(first_error)
                    if hdmi_evidence_probe_action(Some(first_error.kind()), false)
                        == HdmiEvidenceProbeAction::RetryNew =>
                {
                    self.reset_spi_transport();
                    self.read_diagnostic_words_once(command, magic, label)
                        .map_err(|retry_error| {
                            io::Error::new(
                                first_error.kind(),
                                format!(
                                    "{label} failed after retry: first={first_error}; retry={retry_error}"
                                ),
                            )
                        })
                }
                result => result,
            }
        }

        fn read_diagnostic_words_once<const N: usize>(
            &mut self,
            command: u16,
            magic: u16,
            label: &str,
        ) -> io::Result<[u16; N]> {
            let result = (|| {
                let (magic_hi, magic_lo) = self.cmd_capture(command)?;
                match classify_diagnostic_ack(magic_hi, magic_lo, magic) {
                    HdmiEvidenceAck::Present => {}
                    HdmiEvidenceAck::Unsupported => {
                        return Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            format!(
                                "{label} command 0x{command:02x} is unsupported: ack_high=0x{magic_hi:04x} ack_low=0x{magic_lo:04x}"
                            ),
                        ));
                    }
                    HdmiEvidenceAck::Invalid => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "{label} acknowledgement is malformed: ack_high=0x{magic_hi:04x} ack_low=0x{magic_lo:04x}"
                            ),
                        ));
                    }
                }
                let mut words = [0u16; N];
                for word in &mut words {
                    *word = self.spi_capture(0)?.1;
                }
                Ok(words)
            })();
            self.disable_io();
            result
        }

        fn read_required_diagnostic_words<const N: usize>(
            &mut self,
            command: u16,
            magic: u16,
            label: &str,
        ) -> io::Result<[u16; N]> {
            self.read_diagnostic_words(command, magic, label)
                .map_err(|error| {
                    if error.kind() == io::ErrorKind::NotFound {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("{label} disappeared from a supported video-path record set"),
                        )
                    } else {
                        error
                    }
                })
        }

        fn read_hdmi_path_activity_window(&mut self) -> io::Result<HdmiPathActivityWindow> {
            use mister_magik_video_diagnostics_contract as contract;
            let final_first =
                contract::decode_hdmi_final_path_activity(&self.read_diagnostic_words::<{
                    contract::HDMI_FINAL_PATH_ACTIVITY_WORDS
                }>(
                    contract::GET_HDMI_FINAL_PATH_ACTIVITY,
                    contract::HDMI_FINAL_PATH_ACTIVITY_MAGIC,
                    "HDMI final path activity",
                )?)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            let raw_first = contract::decode_hdmi_scaler_raw_activity(
                &self
                    .read_required_diagnostic_words::<{ contract::HDMI_SCALER_RAW_ACTIVITY_WORDS }>(
                        contract::GET_HDMI_SCALER_RAW_ACTIVITY,
                        contract::HDMI_SCALER_RAW_ACTIVITY_MAGIC,
                        "HDMI raw scaler activity",
                    )?,
            )
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            let post_first =
                contract::decode_hdmi_post_osd_activity(&self.read_required_diagnostic_words::<{
                    contract::HDMI_POST_OSD_ACTIVITY_WORDS
                }>(
                    contract::GET_HDMI_POST_OSD_ACTIVITY,
                    contract::HDMI_POST_OSD_ACTIVITY_MAGIC,
                    "HDMI post-OSD activity",
                )?)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            let avalon_first = contract::decode_hdmi_avalon_liveness_activity(
                &self.read_required_diagnostic_words::<{ contract::HDMI_AVALON_LIVENESS_ACTIVITY_WORDS }>(
                    contract::GET_HDMI_AVALON_LIVENESS_ACTIVITY,
                    contract::HDMI_AVALON_LIVENESS_ACTIVITY_MAGIC,
                    "HDMI Avalon liveness activity",
                )?,
            )
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            let scaler_fetch_first = match self
                .read_diagnostic_words::<{ contract::HDMI_SCALER_FETCH_ACTIVITY_WORDS }>(
                    contract::GET_HDMI_SCALER_FETCH_ACTIVITY,
                    contract::HDMI_SCALER_FETCH_ACTIVITY_MAGIC,
                    "HDMI scaler fetch activity",
                ) {
                Ok(words) => Some(
                    contract::decode_hdmi_scaler_fetch_activity(&words)
                        .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?,
                ),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };

            let sample_started = Instant::now();
            thread::sleep(Duration::from_millis(50));

            let final_second = contract::decode_hdmi_final_path_activity(
                &self
                    .read_required_diagnostic_words::<{ contract::HDMI_FINAL_PATH_ACTIVITY_WORDS }>(
                        contract::GET_HDMI_FINAL_PATH_ACTIVITY,
                        contract::HDMI_FINAL_PATH_ACTIVITY_MAGIC,
                        "HDMI final path activity",
                    )?,
            )
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            let raw_second = contract::decode_hdmi_scaler_raw_activity(
                &self
                    .read_required_diagnostic_words::<{ contract::HDMI_SCALER_RAW_ACTIVITY_WORDS }>(
                        contract::GET_HDMI_SCALER_RAW_ACTIVITY,
                        contract::HDMI_SCALER_RAW_ACTIVITY_MAGIC,
                        "HDMI raw scaler activity",
                    )?,
            )
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            let post_second =
                contract::decode_hdmi_post_osd_activity(&self.read_required_diagnostic_words::<{
                    contract::HDMI_POST_OSD_ACTIVITY_WORDS
                }>(
                    contract::GET_HDMI_POST_OSD_ACTIVITY,
                    contract::HDMI_POST_OSD_ACTIVITY_MAGIC,
                    "HDMI post-OSD activity",
                )?)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            let avalon_second = contract::decode_hdmi_avalon_liveness_activity(
                &self.read_required_diagnostic_words::<{ contract::HDMI_AVALON_LIVENESS_ACTIVITY_WORDS }>(
                    contract::GET_HDMI_AVALON_LIVENESS_ACTIVITY,
                    contract::HDMI_AVALON_LIVENESS_ACTIVITY_MAGIC,
                    "HDMI Avalon liveness activity",
                )?,
            )
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            let scaler_fetch_second = if scaler_fetch_first.is_some() {
                Some(
                    contract::decode_hdmi_scaler_fetch_activity(
                        &self.read_required_diagnostic_words::<{
                            contract::HDMI_SCALER_FETCH_ACTIVITY_WORDS
                        }>(
                            contract::GET_HDMI_SCALER_FETCH_ACTIVITY,
                            contract::HDMI_SCALER_FETCH_ACTIVITY_MAGIC,
                            "HDMI scaler fetch activity",
                        )?,
                    )
                    .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?,
                )
            } else {
                None
            };

            Ok(HdmiPathActivityWindow {
                final_first,
                final_second,
                raw_first,
                raw_second,
                post_first,
                post_second,
                avalon_first,
                avalon_second,
                scaler_fetch_first,
                scaler_fetch_second,
                sample_interval_us: sample_started.elapsed().as_micros() as u64,
            })
        }

        fn read_legacy_video_diagnostics_once(
            &mut self,
        ) -> io::Result<LegacyVideoDiagnosticsReadout> {
            use mister_magik_video_diagnostics_contract as contract;

            let control_before = self
                .read_video_diagnostics_record::<{ contract::VIDEO_DIAGNOSTICS_CONTROL_WORDS }>(
                    contract::GET_VIDEO_DIAGNOSTICS_CONTROL,
                    contract::VIDEO_DIAGNOSTICS_CONTROL_MAGIC,
                )?;
            let avalon = self
                .read_video_diagnostics_record::<{ contract::VIDEO_DIAGNOSTICS_AVALON_WORDS }>(
                    contract::GET_VIDEO_DIAGNOSTICS_AVALON,
                    contract::VIDEO_DIAGNOSTICS_AVALON_MAGIC,
                )?;
            let output = self
                .read_video_diagnostics_record::<{ contract::VIDEO_DIAGNOSTICS_OUTPUT_WORDS }>(
                    contract::GET_VIDEO_DIAGNOSTICS_OUTPUT,
                    contract::VIDEO_DIAGNOSTICS_OUTPUT_MAGIC,
                )?;
            let control_after = self
                .read_video_diagnostics_record::<{ contract::VIDEO_DIAGNOSTICS_CONTROL_WORDS }>(
                    contract::GET_VIDEO_DIAGNOSTICS_CONTROL,
                    contract::VIDEO_DIAGNOSTICS_CONTROL_MAGIC,
                )?;
            Ok(LegacyVideoDiagnosticsReadout {
                control_before: contract::decode_control(&control_before)
                    .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?,
                avalon: contract::decode_avalon(&avalon)
                    .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?,
                output: contract::decode_output(&output)
                    .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?,
                control_after: contract::decode_control(&control_after)
                    .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?,
            })
        }

        fn read_video_diagnostics_record<const N: usize>(
            &mut self,
            command: u16,
            magic: u16,
        ) -> io::Result<[u16; N]> {
            let result = (|| {
                let (magic_hi, magic_lo) = self.cmd_capture(command)?;
                if magic_hi != magic && magic_lo != magic {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "video diagnostics command 0x{command:02x} unsupported: ack_high=0x{magic_hi:04x} ack_low=0x{magic_lo:04x}"
                        ),
                    ));
                }
                let mut words = [0u16; N];
                for word in &mut words {
                    *word = self.spi_capture(0)?.1;
                }
                Ok(words)
            })();
            self.disable_io();
            result
        }

        fn read_presentation_telemetry_once(
            &mut self,
        ) -> io::Result<mister_magik_latch_contract::PresentationTelemetry> {
            let result = (|| {
                let (magic_hi, magic_lo) =
                    self.cmd_capture(MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY)?;
                if magic_hi != MAGIK_FBUF_PRESENTATION_TELEMETRY_MAGIC
                    && magic_lo != MAGIK_FBUF_PRESENTATION_TELEMETRY_MAGIC
                {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "presentation telemetry unsupported: ack_high=0x{magic_hi:04x} ack_low=0x{magic_lo:04x}"
                        ),
                    ));
                }
                let mut words =
                    [0u16; mister_magik_latch_contract::V5_PRESENTATION_TELEMETRY_WORDS];
                for word in &mut words {
                    *word = self.spi_capture(0)?.1;
                }
                mister_magik_latch_contract::decode_presentation_telemetry(&words)
                    .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))
            })();
            self.disable_io();
            result
        }

        fn read_latched_fbuf_status_once(
            &mut self,
            protocol: mister_magik_latch_contract::LatchProtocol,
        ) -> io::Result<LatchedFbufStatus> {
            let result = (|| {
                let (magic_hi, magic_lo) = self.cmd_capture(MAGIK_UIO_GET_FBUF_LATCH)?;
                if magic_hi != MAGIK_FBUF_STATUS_MAGIC && magic_lo != MAGIK_FBUF_STATUS_MAGIC {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "latched framebuffer status unsupported: ack_high=0x{magic_hi:04x} ack_low=0x{magic_lo:04x}"
                        ),
                    ));
                }
                let mut words = [0u16; mister_magik_latch_contract::V5_STATUS_WORDS];
                for word in words.iter_mut().take(protocol.status_word_count()) {
                    *word = self.spi_capture(0)?.1;
                }
                let decoded = mister_magik_latch_contract::decode_status(
                    protocol,
                    &words[..protocol.status_word_count()],
                )
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
                Ok(LatchedFbufStatus {
                    active_sequence: decoded.active_seq,
                    pending_sequence: decoded.pending_seq,
                    flags: decoded.flags,
                    flip_count: decoded.flip_count,
                    post_count: decoded.post_count,
                    drop_count: decoded.drop_count,
                    active_base: decoded.base,
                    active_width: decoded.width,
                    active_height: decoded.height,
                    active_stride: decoded.stride,
                    reject_count: decoded.reject_count,
                    active_route_epoch: decoded.active_route_epoch,
                    accepted_sequence: decoded.accepted_seq,
                    active_transaction: decoded.active_transaction,
                    pending_transaction: decoded.pending_transaction,
                    accepted_transaction: decoded.accepted_transaction,
                    crc: decoded.crc,
                })
            })();
            self.disable_io();
            result
        }

        fn negotiate_latch_protocol(
            &mut self,
        ) -> io::Result<mister_magik_latch_contract::LatchProtocol> {
            self.latch_protocol = None;
            self.latch_capabilities = None;
            let result = match self.read_latch_capabilities_once() {
                Err((first, Some(mister_magik_latch_contract::LatchProtocol::V5)))
                    if first.kind() == io::ErrorKind::InvalidData =>
                {
                    self.reset_spi_transport();
                    self.read_latch_capabilities_once()
                        .map_err(|(error, _)| error)
                }
                result => result.map_err(|(error, _)| error),
            };
            let capabilities = result?;
            self.latch_protocol = Some(capabilities.protocol);
            self.latch_capabilities = Some(capabilities);
            Ok(capabilities.protocol)
        }

        fn read_latch_capabilities_once(
            &mut self,
        ) -> Result<
            mister_magik_latch_contract::LatchCapabilities,
            (
                io::Error,
                Option<mister_magik_latch_contract::LatchProtocol>,
            ),
        > {
            let mut observed_protocol = None;
            let result = (|| {
                let (magic_hi, magic_lo) = self.cmd_capture(MAGIK_UIO_GET_FBUF_LATCH_CAPS)?;
                if magic_hi != MAGIK_FBUF_CAPS_MAGIC && magic_lo != MAGIK_FBUF_CAPS_MAGIC {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "latched framebuffer capabilities unsupported: ack_high=0x{magic_hi:04x} ack_low=0x{magic_lo:04x}"
                        ),
                    ));
                }
                let mut words = [0u16; mister_magik_latch_contract::V5_CAPS_WORDS];
                words[0] = self.spi_capture(0)?.1;
                let protocol = mister_magik_latch_contract::LatchProtocol::try_from(words[0])
                    .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
                observed_protocol = Some(protocol);
                for word in words.iter_mut().take(protocol.caps_word_count()).skip(1) {
                    *word = self.spi_capture(0)?.1;
                }
                let capabilities = mister_magik_latch_contract::decode_capabilities(
                    &words[..protocol.caps_word_count()],
                )
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
                if !capabilities.production_ready() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "latched framebuffer capabilities are not production-ready: protocol={} flags=0x{:04x}",
                            capabilities.protocol_version, capabilities.flags
                        ),
                    ));
                }
                Ok(capabilities)
            })();
            self.disable_io();
            result.map_err(|error| (error, observed_protocol))
        }

        fn reset_spi_transport(&mut self) {
            self.gpo = (self.gpo | FPGA_BIT31) & !(FPGA_IO_EN | FPGA_STROBE | 0xffff);
            self.write(self.gpo);
        }

        fn cmd_capture(&mut self, cmd: u16) -> io::Result<(u16, u16)> {
            // gp_out is synchronized into clk_sys. A bare IO_EN-low write can
            // be missed when the next command raises IO_EN immediately, so
            // causally acknowledge the disabled state before starting a new
            // command. The strobe is still acknowledged while UIO is disabled,
            // after every command parser has observed its reset condition.
            self.disable_io();
            self.spi_capture(0)?;
            self.enable_io();
            match self.spi_capture(cmd) {
                Ok(res) => Ok(res),
                Err(err) => {
                    self.disable_io();
                    Err(err)
                }
            }
        }

        fn enable_io(&mut self) {
            self.spi_en(FPGA_IO_EN, true);
        }

        fn disable_io(&mut self) {
            self.spi_en(FPGA_IO_EN, false);
        }

        fn spi_en(&mut self, mask: u32, en: bool) {
            let gpo = self.gpo | FPGA_BIT31;
            self.write(if en { gpo | mask } else { gpo & !mask });
        }

        fn spi_capture(&mut self, word: u16) -> io::Result<(u16, u16)> {
            let gpo = (self.gpo & !(0xffff | FPGA_STROBE)) | word as u32;
            self.write(gpo);
            self.write(gpo | FPGA_STROBE);

            let hi = self.wait_ack(word, true, gpo)?;
            self.write(gpo);
            let lo = self.wait_ack(word, false, gpo)?;
            Ok((hi, lo))
        }

        fn wait_ack(&mut self, word: u16, high: bool, gpo: u32) -> io::Result<u16> {
            for _ in 0..FPGA_SPIN_LIMIT {
                let g = self.read();
                if (g & FPGA_ACK != 0) == high {
                    return Ok(g as u16);
                }
            }
            if high {
                self.write(gpo);
            }
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "FPGA SPI timeout waiting for ACK {} on word 0x{word:04x}",
                    if high { "high" } else { "low" }
                ),
            ))
        }

        fn write(&mut self, value: u32) {
            self.gpo = value;
            unsafe {
                std::ptr::write_volatile(self.base.add(FPGA_GPO_OFF).cast::<u32>(), value);
            }
        }

        fn read(&self) -> u32 {
            unsafe { std::ptr::read_volatile(self.base.add(FPGA_GPI_OFF).cast::<u32>()) }
        }
    }

    impl Drop for FpgaIo {
        fn drop(&mut self) {
            unsafe {
                libc::munmap(self.base.cast::<libc::c_void>(), FPGA_MGR_LEN);
            }
        }
    }

    fn framebuffer_capture_raw(
        request_received: Instant,
        request_received_monotonic_us: u64,
        started: Instant,
        boot_id: u64,
        lz4: bool,
    ) -> Result<RawFramebufferCapture, String> {
        let operation_started_monotonic_us = monotonic_us_now();
        let start = Instant::now();
        let request_received_uptime_ms =
            request_received.duration_since(started).as_millis() as u64;
        let dispatch_us = elapsed_us(request_received);
        let read_t = Instant::now();
        let capture = read_framebuffer_capture()?;
        let raw_read_us = elapsed_us(read_t);
        let geometry = capture.geometry;
        let raw = capture.raw;
        let source_json = capture.source.json();
        let source_label = capture.source.label();
        let authoritative_scanout = capture.source.authoritative_scanout();
        let content_scan_t = Instant::now();
        let (content_nonzero_bytes, content_varied) = framebuffer_content_stats(&raw, geometry);
        let content_scan_us = elapsed_us(content_scan_t);
        let geometry_us = 0;
        let lz4_t = Instant::now();
        let payload = if lz4 {
            lz4_flex::compress_prepend_size(&raw)
        } else {
            raw.clone()
        };
        let lz4_encode_us = if lz4 { elapsed_us(lz4_t) } else { 0 };
        let total_us = elapsed_us(start);
        let operation_ended_monotonic_us = monotonic_us_now();
        let raw_bytes = raw.len() as u64;
        let payload_bytes = payload.len() as u64;
        let phases = json!({
            "raw_framebuffer_capture": raw_read_us,
            "content_scan": content_scan_us,
            "lz4": lz4_encode_us,
        });
        let mut result = json!({
            "schema": "mister-magik-framebuffer-raw-stream-v2",
            "boot_id": boot_id,
            "source": source_label,
            "capture_source": source_json,
            "authoritative_scanout": authoritative_scanout,
            "width": geometry.width,
            "height": geometry.height,
            "stride": geometry.stride,
            "bpp": geometry.bpp,
            "format": if geometry.bpp == 16 { "rgb565-le" } else { "bgrx8888" },
            "encoding": if lz4 { "lz4-block-size-prepended" } else { "raw" },
            "raw_bytes": raw.len(),
            "payload_bytes": payload.len(),
            "content_nonzero_bytes": content_nonzero_bytes,
            "content_varied": content_varied,
            "elapsed_ms": total_us / 1000,
            "timings": {
                "request_received_uptime_ms": request_received_uptime_ms,
                "dispatch_us": dispatch_us,
                "geometry_us": geometry_us,
                "raw_read_us": raw_read_us,
                "content_scan_us": content_scan_us,
                "lz4_encode_us": lz4_encode_us,
                "total_us": total_us,
            },
        });
        attach_io_operation_evidence(
            &mut result,
            io_operation_evidence(IoOperationEvidence {
                operation: if lz4 {
                    "framebuffer_lz4_capture"
                } else {
                    "framebuffer_raw_capture"
                },
                request_received_monotonic_us,
                operation_started_monotonic_us,
                operation_ended_monotonic_us,
                bytes_read: raw_bytes,
                bytes_written: payload_bytes,
                peak_buffer_ownership_bytes: raw_bytes.saturating_add(payload_bytes),
                peak_rss_kb: peak_rss_kb(),
                phases_us: phases,
            }),
        );
        Ok(RawFramebufferCapture { result, payload })
    }

    fn framebuffer_geometry() -> Result<FramebufferGeometry, String> {
        let virtual_size = read_trimmed("/sys/class/graphics/fb0/virtual_size")
            .unwrap_or_else(|| "960,540".to_string());
        let (width, height) = parse_virtual_size(&virtual_size).unwrap_or((960, 540));
        let bpp = read_trimmed("/sys/class/graphics/fb0/bits_per_pixel")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(16);
        let bytes_per_pixel = match bpp {
            16 => 2,
            32 => 4,
            _ => return Err(format!("unsupported framebuffer bpp: {bpp}")),
        };
        let packed_stride = width
            .checked_mul(bytes_per_pixel)
            .ok_or_else(|| "framebuffer stride overflow".to_string())?;
        let stride = read_trimmed("/sys/class/graphics/fb0/stride")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(packed_stride);
        if stride < packed_stride {
            return Err(format!(
                "framebuffer stride {stride} is smaller than packed row {packed_stride}"
            ));
        }
        Ok(FramebufferGeometry {
            width,
            height,
            stride,
            bpp,
        })
    }

    fn parse_virtual_size(text: &str) -> Option<(usize, usize)> {
        let (width, height) = text.trim().split_once(',')?;
        Some((width.parse().ok()?, height.parse().ok()?))
    }

    fn png_geometry(geometry: FramebufferGeometry) -> png_capture::Geometry {
        png_capture::Geometry {
            width: geometry.width,
            height: geometry.height,
            stride: geometry.stride,
            bpp: geometry.bpp,
        }
    }

    fn framebuffer_png(
        raw: &[u8],
        geometry: FramebufferGeometry,
    ) -> Result<png_capture::EncodeResult, String> {
        png_capture::encode(raw, png_geometry(geometry))
    }

    fn logical_rgba_len(geometry: FramebufferGeometry) -> Result<usize, String> {
        png_capture::logical_rgba_len(png_geometry(geometry))
    }

    fn encode_hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }

    fn elapsed_us(start: Instant) -> u64 {
        start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }

    fn main_generation(status: &Value) -> Option<u64> {
        status.get("main_generation").and_then(Value::as_u64)
    }

    fn main_process_running() -> bool {
        read_pid_list("MiSTer_MagiKDev")
            .as_array()
            .is_some_and(|pids| !pids.is_empty())
            || read_pid_list("MiSTer_MagiK")
                .as_array()
                .is_some_and(|pids| !pids.is_empty())
    }

    fn current_main_ready() -> Result<Value, String> {
        let status = read_json_value("/tmp/mister-magik/main-status.json");
        let ready = status.get("command_channel").and_then(Value::as_str) == Some("ready");
        let current_pid = status
            .get("pid")
            .and_then(Value::as_u64)
            .is_some_and(|pid| {
                read_pid_list("MiSTer_MagiKDev")
                    .as_array()
                    .is_some_and(|pids| pids.iter().any(|p| p.as_u64() == Some(pid)))
                    || read_pid_list("MiSTer_MagiK")
                        .as_array()
                        .is_some_and(|pids| pids.iter().any(|p| p.as_u64() == Some(pid)))
            });
        if ready && current_pid {
            Ok(status)
        } else {
            Err("command_channel_unavailable".to_string())
        }
    }

    fn write_main_command_nonblocking(command: &str) -> Result<(), String> {
        let mut fifo = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open("/dev/MiSTer_cmd")
            .map_err(|err| format!("command_channel_unavailable: {err}"))?;
        fifo.write_all(format!("{command}\n").as_bytes())
            .map_err(|err| format!("command_write_failed: {err}"))
    }

    const MAIN_COMMAND_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
    const MAIN_COMMAND_ACK_TIMEOUT: Duration = Duration::from_secs(20);

    fn acquire_main_command_lock(file: &File) -> Result<(), String> {
        let started = Instant::now();
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock {
                return Err(format!("command_lock_failed: {error}"));
            }
            if started.elapsed() >= MAIN_COMMAND_LOCK_TIMEOUT {
                return Err("command_lock_timed_out".to_string());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn send_main_command_acknowledged(command: &str, generation: u64) -> Result<String, String> {
        let command_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open("/tmp/mister-magik/command-operation.lock")
            .map_err(|err| format!("command_lock_unavailable: {err}"))?;
        acquire_main_command_lock(&command_lock)?;
        let mut reply = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open("/dev/MiSTer_cmd_reply")
            .map_err(|err| format!("reply_channel_unavailable: {err}"))?;
        let mut discard = [0u8; 256];
        while reply.read(&mut discard).is_ok_and(|count| count > 0) {}
        let mut bytes = Vec::with_capacity(128);
        let mut heartbeat = current_main_ready()?
            .get("ts_boot_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let mut heartbeat_seen = Instant::now();
        let acknowledgement_started = Instant::now();
        write_main_command_nonblocking(command)?;
        loop {
            let mut chunk = [0u8; 128];
            match reply.read(&mut chunk) {
                Ok(0) => return Err("command_channel_closed".to_string()),
                Ok(count) => {
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
                        return String::from_utf8(bytes[..end].to_vec())
                            .map_err(|_| "invalid reply encoding".to_string());
                    }
                    if bytes.len() > 512 {
                        return Err("reply too long".to_string());
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(format!("reply_read_failed: {err}")),
            }
            if acknowledgement_started.elapsed() >= MAIN_COMMAND_ACK_TIMEOUT {
                return Err("command_acknowledgement_timed_out".to_string());
            }
            let status = read_json_value("/tmp/mister-magik/main-status.json");
            if main_generation(&status).is_some_and(|current| current != generation) {
                return Err("command_channel_restarted".to_string());
            }
            if !main_process_running() {
                return Err("command_channel_closed".to_string());
            }
            let current_heartbeat = status
                .get("ts_boot_ms")
                .and_then(Value::as_u64)
                .unwrap_or(heartbeat);
            if current_heartbeat != heartbeat {
                heartbeat = current_heartbeat;
                heartbeat_seen = Instant::now();
            } else if heartbeat_seen.elapsed() >= Duration::from_secs(10) {
                return Err("main_heartbeat_stopped".to_string());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn magik_acknowledged_action(action: &str, args: &Value) -> Result<Value, String> {
        let request_monotonic_us = monotonic_us_now();
        let operation_id = args
            .get("operation_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing operation_id".to_string())?;
        if operation_id.is_empty()
            || operation_id.len() > 128
            || !operation_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err("invalid operation_id".to_string());
        }
        let cache = MAGIK_OPERATION_RESULTS.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(result) = cache
            .lock()
            .map_err(|_| "operation cache poisoned")?
            .get(operation_id)
            .cloned()
        {
            if result.get("terminal_reason").and_then(Value::as_str) == Some("failed") {
                return Err(result
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("cached operation failure")
                    .to_string());
            }
            return Ok(result);
        }
        let started = Instant::now();
        let ready = current_main_ready()?;
        let before_generation = main_generation(&ready).unwrap_or(0);
        if let Some(expected) = args.get("expected_generation").and_then(Value::as_u64)
            && expected != before_generation
        {
            return Err(format!(
                "stale_main_generation expected={expected} actual={before_generation}"
            ));
        }
        if action == "suspend"
            && ready.get("launcher_state").and_then(Value::as_str) == Some("LauncherSuspended")
        {
            let result = json!({
                "operation_id": operation_id,
                "action": action,
                "before_generation": before_generation,
                "after_generation": before_generation,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "request_monotonic_us": request_monotonic_us,
                "acknowledged_monotonic_us": monotonic_us_now(),
                "terminal_reason": "already-satisfied",
                "main_status": ready,
            });
            cache
                .lock()
                .map_err(|_| "operation cache poisoned")?
                .insert(operation_id.to_string(), result.clone());
            return Ok(result);
        }
        let command = match action {
            "suspend" => "mister_magik_suspend",
            "resume" => "mister_magik_resume",
            "restart-launcher" => "mister_magik_restart_launcher",
            "return-to-launcher" => "mister_magik_return_to_launcher",
            "exit-to-menu" => "mister_magik_exit_to_menu",
            "launch" => {
                let target = args
                    .get("target")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "missing launch target".to_string())?;
                if target.is_empty() || target.len() > 2048 || target.contains(['\n', '\r']) {
                    return Err("invalid launch target".to_string());
                }
                return magik_acknowledged_launch(
                    operation_id,
                    before_generation,
                    target,
                    started,
                    request_monotonic_us,
                    cache,
                );
            }
            _ => return Err(format!("unsupported magik action: {action}")),
        };
        let reply = match send_main_command_acknowledged(command, before_generation) {
            Ok(reply) => reply,
            Err(error) => {
                cache_operation_failure(cache, operation_id, action, &error, started.elapsed());
                return Err(error);
            }
        };
        if let Err(error) = require_ok_main_reply(&reply) {
            cache_operation_failure(cache, operation_id, action, &error, started.elapsed());
            return Err(error);
        }
        let final_status = read_json_value("/tmp/mister-magik/main-status.json");
        let acknowledged_monotonic_us = monotonic_us_now();
        let result = json!({
            "operation_id": operation_id,
            "action": action,
            "command": command,
            "before_generation": before_generation,
            "after_generation": main_generation(&final_status),
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "request_monotonic_us": request_monotonic_us,
            "acknowledged_monotonic_us": acknowledged_monotonic_us,
            "terminal_reason": "acknowledged",
            "main_status": final_status,
        });
        let mut results = cache.lock().map_err(|_| "operation cache poisoned")?;
        if results.len() >= 128 {
            results.clear();
        }
        results.insert(operation_id.to_string(), result.clone());
        Ok(result)
    }

    fn cache_operation_failure(
        cache: &Mutex<HashMap<String, Value>>,
        operation_id: &str,
        action: &str,
        error: &str,
        elapsed: Duration,
    ) {
        if let Ok(mut results) = cache.lock() {
            if results.len() >= 128 {
                results.clear();
            }
            results.insert(operation_id.to_string(), json!({"operation_id":operation_id,"action":action,"terminal_reason":"failed","error":error,"elapsed_ms":elapsed.as_millis() as u64}));
        }
    }

    fn magik_acknowledged_launch(
        operation_id: &str,
        before_generation: u64,
        target: &str,
        started: Instant,
        request_monotonic_us: u64,
        cache: &Mutex<HashMap<String, Value>>,
    ) -> Result<Value, String> {
        let command = format!("mister_magik_launch {target}");
        let reply = match send_main_command_acknowledged(&command, before_generation) {
            Ok(reply) => reply,
            Err(error) => {
                cache_operation_failure(cache, operation_id, "launch", &error, started.elapsed());
                return Err(error);
            }
        };
        if let Err(error) = require_ok_main_reply(&reply) {
            cache_operation_failure(cache, operation_id, "launch", &error, started.elapsed());
            return Err(error);
        }
        let final_status = read_json_value("/tmp/mister-magik/main-status.json");
        let result = json!({"operation_id":operation_id,"action":"launch","command":command,"before_generation":before_generation,"after_generation":main_generation(&final_status),"elapsed_ms":started.elapsed().as_millis() as u64,"request_monotonic_us":request_monotonic_us,"acknowledged_monotonic_us":monotonic_us_now(),"terminal_reason":"acknowledged","main_status":final_status});
        let mut results = cache.lock().map_err(|_| "operation cache poisoned")?;
        if results.len() >= 128 {
            results.clear();
        }
        results.insert(operation_id.to_string(), result.clone());
        Ok(result)
    }

    fn magik_status_json(action: &str, command: Option<String>, settle_ms: Option<u64>) -> Value {
        let main_dev_pids = read_pid_list("MiSTer_MagiKDev");
        let main_public_pids = read_pid_list("MiSTer_MagiK");
        let launcher_pids = read_pid_list("mister-magik-fb");
        let main_status = read_json_value("/tmp/mister-magik/main-status.json");
        let slint_status = read_json_value("/tmp/mister-magik/status.json");
        let slint_status_current = status_pid_matches(&slint_status, &launcher_pids);
        json!({
            "action": action,
            "command": command,
            "settle_ms": settle_ms,
            "processes": {
                "MiSTer_MagiKDev": main_dev_pids,
                "MiSTer_MagiK": main_public_pids,
                "mister-magik-fb": launcher_pids,
            },
            "files": {
                "main_status": main_status,
                "slint_status": slint_status,
                "slint_status_current": slint_status_current,
                "core_name": read_trimmed_text_value("/tmp/CORENAME"),
                "rbf_name": read_trimmed_text_value("/tmp/RBFNAME"),
            }
        })
    }

    pub(super) fn read_trimmed_text_value(path: &str) -> Value {
        fs::read_to_string(path)
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
            .map(Value::String)
            .unwrap_or(Value::Null)
    }

    fn read_text_value(path: &str) -> Value {
        fs::read_to_string(path)
            .map(Value::String)
            .unwrap_or(Value::Null)
    }

    fn read_json_value(path: &str) -> Value {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null)
    }

    fn crash_reports_json() -> Value {
        let recent = recent_crash_report_paths(5)
            .into_iter()
            .map(|path| {
                let path_text = path.to_string_lossy().to_string();
                let report = fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or(Value::Null);
                json!({
                    "path": path_text,
                    "report": report,
                })
            })
            .collect::<Vec<_>>();
        json!({
            "dir": CRASH_DIR,
            "latest_path": LATEST_CRASH_REPORT,
            "latest": read_json_value(LATEST_CRASH_REPORT),
            "recent": recent,
        })
    }

    fn catalog_failure_reports_json() -> Value {
        let mut latest_reports = CATALOG_FAILURE_DIRS
            .iter()
            .filter_map(|dir| {
                let path = Path::new(dir).join("latest.json");
                let report = read_json_value(path.to_string_lossy().as_ref());
                report
                    .is_object()
                    .then(|| (path.to_string_lossy().to_string(), report))
            })
            .collect::<Vec<_>>();
        latest_reports.sort_by_key(|(_, report)| {
            report
                .get("ts_unix_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        });
        let latest = latest_reports
            .pop()
            .map(|(path, report)| json!({"path": path, "report": report}))
            .unwrap_or(Value::Null);
        let mut recent_paths = CATALOG_FAILURE_DIRS
            .iter()
            .flat_map(|dir| recent_catalog_failure_paths(Path::new(dir), 5))
            .collect::<Vec<_>>();
        recent_paths.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
        recent_paths.dedup();
        recent_paths.reverse();
        recent_paths.truncate(5);
        let recent = recent_paths
            .into_iter()
            .map(|path| {
                let report = read_json_value(path.to_string_lossy().as_ref());
                json!({
                    "path": path,
                    "report_id": report.get("report_id").cloned().unwrap_or(Value::Null),
                    "ts_unix_ms": report.get("ts_unix_ms").cloned().unwrap_or(Value::Null),
                    "code": report.pointer("/failure/code").cloned().unwrap_or(Value::Null),
                })
            })
            .collect::<Vec<_>>();
        json!({
            "latest": latest,
            "recent": recent,
        })
    }

    fn latest_diagnostic_report(paths: &[&str], timestamp_field: &str) -> Value {
        let mut reports = paths
            .iter()
            .filter_map(|path| {
                let report = read_json_value(path);
                report.is_object().then(|| ((*path).to_string(), report))
            })
            .collect::<Vec<_>>();
        reports.sort_by_key(|(_, report)| {
            report
                .get(timestamp_field)
                .and_then(Value::as_u64)
                .unwrap_or(0)
        });
        reports
            .pop()
            .map(|(path, report)| json!({"path": path, "report": report}))
            .unwrap_or(Value::Null)
    }

    fn current_latch_failure_report() -> Value {
        for pointer_path in LATCH_IDENTITY_PATHS {
            let pointer = read_json_value(pointer_path);
            let Some(relative) = pointer.get("latest_relative_path").and_then(Value::as_str) else {
                continue;
            };
            if relative.starts_with('/') || relative.split('/').any(|part| part == "..") {
                continue;
            }
            let Some(root) = Path::new(pointer_path).parent() else {
                continue;
            };
            let report_path = root.join(relative);
            let report = read_json_value(report_path.to_string_lossy().as_ref());
            if report.get("schema").and_then(Value::as_str)
                != Some("mister-magik-latch-failure-report-v2")
                || report.get("identity") != pointer.get("identity")
            {
                continue;
            }
            return json!({
                "path": report_path,
                "identity_pointer": pointer_path,
                "report": report,
            });
        }
        Value::Null
    }

    fn recent_catalog_failure_paths(dir: &Path, limit: usize) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("report-catalog-") && name.ends_with(".json")
                    })
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths.reverse();
        paths.truncate(limit);
        paths
    }

    fn recent_crash_report_paths(limit: usize) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(CRASH_DIR) else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        let latest_report_id = read_json_value(LATEST_CRASH_REPORT)
            .get("report_id")
            .and_then(Value::as_str)
            .map(|report_id| format!("{report_id}.json"));
        if let Some(name) = latest_report_id.as_deref() {
            paths.push(Path::new(CRASH_DIR).join(name));
        }
        let mut other_paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("report-")
                            && name.ends_with(".json")
                            && Some(name) != latest_report_id.as_deref()
                    })
            })
            .collect::<Vec<_>>();
        other_paths.sort();
        paths.extend(other_paths.into_iter().rev());
        paths.into_iter().take(limit).collect::<Vec<_>>()
    }

    fn current_status_pid(status: &Value, pids: &Value) -> Option<u64> {
        let status_pid = status.get("pid").and_then(Value::as_u64)?;
        pids.as_array()
            .is_some_and(|pids| pids.iter().any(|pid| pid.as_u64() == Some(status_pid)))
            .then_some(status_pid)
    }

    pub(super) fn launcher_ui_pid(status: &Value, pids: &Value) -> Option<u64> {
        current_status_pid(status, pids).or_else(|| {
            let pids = pids.as_array()?;
            (pids.len() == 1).then(|| pids[0].as_u64()).flatten()
        })
    }

    fn status_pid_matches(status: &Value, pids: &Value) -> bool {
        current_status_pid(status, pids).is_some()
    }

    fn tail_text_value(path: &str, n: usize) -> Value {
        let Ok(text) = fs::read_to_string(path) else {
            return Value::Null;
        };
        let lines: Vec<_> = text.lines().collect();
        let start = lines.len().saturating_sub(n);
        Value::String(lines[start..].join("\n"))
    }

    fn command_text_value(program: &str, args: &[&str]) -> Value {
        match std::process::Command::new(program).args(args).output() {
            Ok(output) => {
                let mut text = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.trim().is_empty() {
                    text.push_str("\n[stderr]\n");
                    text.push_str(&stderr);
                }
                Value::String(text)
            }
            Err(err) => Value::String(format!("error: {err}")),
        }
    }

    fn schedule_reboot(args: Value) -> Result<String, String> {
        let mode = args
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("raw")
            .to_string();
        match mode.as_str() {
            "raw" | "supervised" => {}
            _ => return Err(format!("unsupported reboot mode: {mode}")),
        }
        let thread_mode = mode.clone();
        append_persistent_log_line(format!("reboot_scheduled mode={mode}"));
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let result = if thread_mode == "supervised" {
                current_main_ready()
                    .and_then(|status| {
                        main_generation(&status)
                            .ok_or_else(|| "main_generation_unavailable".to_string())
                    })
                    .and_then(|generation| {
                        send_main_command_acknowledged("mister_magik_reboot", generation)
                    })
                    .and_then(|reply| require_ok_main_reply(&reply))
            } else {
                std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg("nohup /sbin/reboot >/dev/null 2>&1 &")
                    .spawn()
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            };
            if let Err(err) = result {
                append_log_line(format!(
                    "reboot_schedule_error mode={thread_mode} err={err}"
                ));
            }
        });
        Ok(mode)
    }

    fn read_routes() -> Value {
        let routes: Vec<Value> = fs::read_to_string("/proc/net/route")
            .ok()
            .map(|text| {
                text.lines()
                    .skip(1)
                    .map(|line| {
                        let fields: Vec<_> = line.split_whitespace().collect();
                        json!({
                            "iface": fields.first().copied().unwrap_or(""),
                            "destination": fields.get(1).copied().unwrap_or(""),
                            "gateway": fields.get(2).copied().unwrap_or(""),
                            "flags": fields.get(3).copied().unwrap_or(""),
                            "metric": fields.get(6).copied().unwrap_or(""),
                            "mask": fields.get(7).copied().unwrap_or(""),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Value::Array(routes)
    }

    fn read_arp_entries() -> Value {
        let entries: Vec<Value> = fs::read_to_string("/proc/net/arp")
            .ok()
            .map(|text| {
                text.lines()
                    .skip(1)
                    .map(|line| {
                        let fields: Vec<_> = line.split_whitespace().collect();
                        json!({
                            "ip": fields.first().copied().unwrap_or(""),
                            "flags": fields.get(2).copied().unwrap_or(""),
                            "mac": fields.get(3).copied().unwrap_or(""),
                            "device": fields.get(5).copied().unwrap_or(""),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Value::Array(entries)
    }

    fn read_netdev_stats_value(iface: &str) -> Value {
        match read_netdev_stats_fields(iface) {
            Some(fields) => json!({
                "rx_bytes": fields[0],
                "rx_packets": fields[1],
                "tx_bytes": fields[8],
                "tx_packets": fields[9],
            }),
            None => Value::Null,
        }
    }

    fn read_pid_list(name: &str) -> Value {
        let pids: Vec<Value> = read_pidof(name)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|pid| pid.parse::<u64>().ok())
            .map(Value::from)
            .collect();
        Value::Array(pids)
    }

    fn active_magik_main_name() -> Option<&'static str> {
        ["MiSTer_MagiKDev", "MiSTer_MagiK"]
            .into_iter()
            .find(|name| read_pidof(name).is_some())
    }

    fn append_log_line(msg: String) {
        let line = format!("{} agent {msg}", stamp());
        if let Some(ring) = LOG_RING.get() {
            record_log_line(ring, &line);
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(LOG) {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }

    fn append_persistent_log_line(msg: String) {
        append_log_line(msg.clone());
        let _ = fs::create_dir_all(BOOTLOG_DIR);
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(PLOG) {
            let _ = writeln!(file, "{} agent {msg}", stamp());
            let _ = file.flush();
            let _ = file.sync_all();
        }
    }

    fn snapshot(boot_id: u64, log: &mut Logger) {
        let carrier = read_trimmed("/sys/class/net/eth0/carrier").unwrap_or_else(|| "?".into());
        let operstate = read_trimmed("/sys/class/net/eth0/operstate").unwrap_or_else(|| "?".into());
        let sshd_pid = read_pidof("sshd").unwrap_or_else(|| "none".into());
        if sshd_pid != "none" {
            timeline_record_once("sshd_seen", format!("pid={sshd_pid}"));
        }
        if let Some(name) = active_magik_main_name()
            && let Some(pid) = read_pidof(name)
        {
            timeline_record_once("magik_main_seen", format!("name={name} pid={pid}"));
        }
        if let Some(pid) = read_pidof("mister-magik-fb") {
            timeline_record_once("magik_launcher_seen", format!("pid={pid}"));
        }
        let stats = read_netdev_stats(IFACE).unwrap_or_default();
        if let Some(fields) = read_netdev_stats_fields(IFACE) {
            if fields[1] > 0 {
                timeline_record_once(
                    "first_rx",
                    format!("rx_bytes={} rx_packets={}", fields[0], fields[1]),
                );
            }
            if fields[9] > 0 {
                timeline_record_once(
                    "first_tx",
                    format!("tx_bytes={} tx_packets={}", fields[8], fields[9]),
                );
            }
        }
        log.line(format!(
            "snapshot boot={boot_id} carrier={carrier} operstate={operstate} sshd_pid={sshd_pid} {stats}"
        ));
        if let Some(route) = read_trimmed("/proc/net/route") {
            for line in route.lines().take(4) {
                log.line(format!("route {line}"));
            }
        }
    }

    fn read_netdev_stats(iface: &str) -> Option<String> {
        let fields = read_netdev_stats_fields(iface)?;
        Some(format!(
            "rx_bytes={} rx_packets={} tx_bytes={} tx_packets={}",
            fields[0], fields[1], fields[8], fields[9]
        ))
    }

    fn read_netdev_stats_fields(iface: &str) -> Option<[u64; 16]> {
        let text = fs::read_to_string("/proc/net/dev").ok()?;
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix(&format!("{iface}:")) {
                let fields: Vec<u64> = rest
                    .split_whitespace()
                    .filter_map(|field| field.parse().ok())
                    .collect();
                if fields.len() >= 16 {
                    let mut values = [0u64; 16];
                    values.copy_from_slice(&fields[..16]);
                    return Some(values);
                }
            }
        }
        None
    }

    fn persist_log(boot_id: u64, log: &mut Logger) {
        thread::sleep(Duration::from_secs(20));
        let _ = fs::create_dir_all(BOOTLOG_DIR);
        let text = log.ring_text();
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(PLOG) {
            let _ = writeln!(
                file,
                "--- agent deferred boot={boot_id} uptime={} ---",
                stamp()
            );
            let _ = writeln!(file, "{text}");
        }
        log.line(format!("persisted boot={boot_id}"));
    }

    fn next_boot_id() -> u64 {
        let _ = fs::create_dir_all(BOOTLOG_DIR);
        let n = fs::read_to_string(SEQ)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        let _ = fs::write(SEQ, n.to_string());
        n
    }

    fn read_pidof(name: &str) -> Option<String> {
        let output = std::process::Command::new("pidof")
            .arg(name)
            .output()
            .ok()?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
        None
    }

    fn read_trimmed(path: &str) -> Option<String> {
        fs::read_to_string(path).ok().map(|s| s.trim().to_string())
    }

    fn stamp() -> String {
        fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_whitespace().next().map(str::to_string))
            .unwrap_or_else(|| "?".into())
    }

    fn uptime_ms_now() -> u64 {
        fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
            .map(|secs| (secs * 1000.0) as u64)
            .unwrap_or(0)
    }

    fn monotonic_us_now() -> u64 {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `ts` is a valid writable timespec and CLOCK_MONOTONIC is a
        // read-only process-independent device clock.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
            return 0;
        }
        u64::try_from(ts.tv_sec)
            .unwrap_or(0)
            .saturating_mul(1_000_000)
            .saturating_add(u64::try_from(ts.tv_nsec).unwrap_or(0) / 1_000)
    }
}

#[cfg(not(target_os = "linux"))]
mod linux {
    pub fn run(_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
        Err("mister-magik-agent can only run on Linux/MiSTer".into())
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if let Err(err) = linux::run(&args) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::ZlibDecoder;
    use std::io::Read;
    #[cfg(target_os = "linux")]
    use std::io::{BufReader, Cursor};
    #[cfg(target_os = "linux")]
    use std::time::Duration;

    #[cfg(target_os = "linux")]
    #[test]
    fn ui_test_startup_timeout_is_bounded_by_session_timeout() {
        assert_eq!(
            linux::ui_test_startup_timeout(1_000),
            Duration::from_secs(1)
        );
        assert_eq!(
            linux::ui_test_startup_timeout(120_000),
            Duration::from_secs(30)
        );
    }

    fn decode_truecolor_png(png: &[u8]) -> (usize, usize, Vec<u8>) {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let mut offset = 8usize;
        let mut width = 0usize;
        let mut height = 0usize;
        let mut idat = Vec::new();
        let mut saw_iend = false;
        while offset < png.len() {
            let length = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
            let tag = &png[offset + 4..offset + 8];
            let data = &png[offset + 8..offset + 8 + length];
            match tag {
                b"IHDR" => {
                    assert_eq!(length, 13);
                    width = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
                    height = u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize;
                    assert_eq!(&data[8..], &[8, 2, 0, 0, 0]);
                }
                b"IDAT" => idat.extend_from_slice(data),
                b"IEND" => saw_iend = true,
                _ => {}
            }
            offset += 12 + length;
        }
        assert_eq!(offset, png.len());
        assert!(saw_iend);
        let mut decoded = Vec::new();
        ZlibDecoder::new(idat.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        (width, height, decoded)
    }

    #[test]
    fn png_capture_streams_rgb565_padded_odd_scanlines_as_truecolor() {
        let geometry = png_capture::Geometry {
            width: 3,
            height: 2,
            stride: 8,
            bpp: 16,
        };
        let mut raw = vec![0xa5; geometry.stride * geometry.height];
        let pixels = [[0xf800u16, 0x07e0, 0x001f], [0xffff, 0x0000, 0x8410]];
        for (y, row) in pixels.iter().enumerate() {
            for (x, pixel) in row.iter().enumerate() {
                raw[y * geometry.stride + x * 2..y * geometry.stride + x * 2 + 2]
                    .copy_from_slice(&pixel.to_le_bytes());
            }
        }

        let encoded = png_capture::encode(&raw, geometry).unwrap();
        let _timing = encoded.timing;
        let (width, height, decoded) = decode_truecolor_png(&encoded.bytes);

        assert_eq!((width, height), (3, 2));
        assert_eq!(
            decoded,
            vec![
                0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 255, 255, 255, 0, 0, 0, 132, 130, 132,
            ]
        );
        assert!(encoded.workspace_peak_bytes < 256);
        assert_eq!(png_capture::logical_rgba_len(geometry).unwrap(), 26);
    }

    #[test]
    fn png_capture_preserves_bgrx_pixels_across_entropy_cases() {
        for raw in [
            vec![0u8; 16],
            vec![
                0, 1, 2, 3, 127, 128, 129, 130, 253, 254, 255, 17, 31, 63, 95, 127,
            ],
        ] {
            let geometry = png_capture::Geometry {
                width: 4,
                height: 1,
                stride: 16,
                bpp: 32,
            };
            let encoded = png_capture::encode(&raw, geometry).unwrap();
            let (_, _, decoded) = decode_truecolor_png(&encoded.bytes);
            let mut expected = vec![0];
            for pixel in raw.as_chunks::<4>().0 {
                expected.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
            }
            assert_eq!(decoded, expected);
        }
    }

    #[test]
    fn png_capture_rejects_malformed_geometry_before_allocation() {
        assert!(
            png_capture::encode(
                &[0; 4],
                png_capture::Geometry {
                    width: 2,
                    height: 1,
                    stride: 3,
                    bpp: 16,
                },
            )
            .err()
            .expect("malformed geometry should fail")
            .contains("stride")
        );
        assert!(
            png_capture::encode(
                &[0; 4],
                png_capture::Geometry {
                    width: 1,
                    height: 2,
                    stride: 4,
                    bpp: 32,
                },
            )
            .err()
            .expect("malformed geometry should fail")
            .contains("expected at least")
        );
        assert!(
            png_capture::encode(
                &[],
                png_capture::Geometry {
                    width: 1,
                    height: 1,
                    stride: 4,
                    bpp: 24,
                },
            )
            .err()
            .expect("malformed geometry should fail")
            .contains("unsupported")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn control_request_rejects_oversized_headers_without_consuming_payload() {
        let oversized = vec![b'x'; linux::MAX_CONTROL_REQUEST_BYTES + 1];
        let error =
            linux::read_control_request(&mut BufReader::new(Cursor::new(oversized)), |_| Ok(()))
                .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

        let mut reader = BufReader::new(Cursor::new(b"{}\npayload".to_vec()));
        assert_eq!(
            linux::read_control_request(&mut reader, |_| Ok(())).unwrap(),
            "{}\n"
        );
        let mut payload = Vec::new();
        reader.read_to_end(&mut payload).unwrap();
        assert_eq!(payload, b"payload");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn control_client_admission_is_bounded_and_reopens_after_drop() {
        let clients = linux::ActiveControlClients::new(2);
        let first = clients.claim().expect("first client");
        let second = clients.claim().expect("second client");
        assert!(clients.claim().is_none());
        drop(first);
        assert!(clients.claim().is_some());
        drop(second);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn presentation_telemetry_json_never_conflates_unavailable_with_zero() {
        let available = linux::presentation_telemetry_json(
            mister_magik_latch_contract::PresentationTelemetry {
                owned_vblank_count: 60,
                presented_vblank_count: 59,
                repeated_vblank_count: 1,
                ownership_loss_count: 0,
                active_sequence: 42,
                flags: 1 << 3,
                crc: 0,
            },
            1_000_000,
        );
        assert_eq!(available["available"], true);
        assert_eq!(available["repeated_vblank_count"], 1);
        assert_eq!(available["captured_monotonic_us"], 1_000_000);

        let unavailable = linux::unavailable_presentation_telemetry("busy".into());
        assert_eq!(unavailable["available"], false);
        assert!(unavailable.get("repeated_vblank_count").is_none());
        assert_eq!(unavailable["error"], "busy");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn analytics_lease_refresh_replaces_complete_content_atomically() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-agent-lease-{}", std::process::id()));
        let path = root.join("realtime-frame-analytics");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&path, "wall\n").unwrap();

        linux::refresh_frame_analytics_lease_at(&path, "process", 7);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "process\n");
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            1,
            "temporary lease file was not renamed away"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn authoritative_capture_failure_never_falls_back_when_slots_exist() {
        let mut fb0_reads = 0;
        let error = select_framebuffer_capture(
            true,
            false,
            || Err::<u8, _>("latch unavailable".to_string()),
            |_| Ok(2),
            || {
                fb0_reads += 1;
                Ok(1)
            },
        )
        .unwrap_err();

        assert_eq!(fb0_reads, 0);
        assert!(error.contains("authoritative scanout capture failed"));
        assert!(error.contains("latch unavailable"));
    }

    #[test]
    fn compatibility_capture_falls_back_to_producer_and_preserves_authoritative_error() {
        let value = select_framebuffer_capture(
            true,
            true,
            || Err::<String, _>("active base is not a hidden slot".to_string()),
            |error| Ok(format!("producer:{error}")),
            || Err("must not read fb0".to_string()),
        )
        .unwrap();

        assert_eq!(
            value,
            "producer:active base is not a hidden slot".to_string()
        );
    }

    #[test]
    fn compatibility_capture_reports_both_failures_when_producer_is_unavailable() {
        let error = select_framebuffer_capture(
            true,
            true,
            || Err::<u8, _>("active base is not a hidden slot".to_string()),
            |_| Err("stream unavailable".to_string()),
            || Ok(1),
        )
        .unwrap_err();

        assert!(error.contains("active base is not a hidden slot"));
        assert!(error.contains("stream unavailable"));
    }

    #[test]
    fn fb0_capture_remains_available_without_scanout_slots() {
        let value = select_framebuffer_capture(
            false,
            false,
            || Err::<u8, _>("must not read latch".to_string()),
            |_| Err("must not read producer".to_string()),
            || Ok(7),
        )
        .unwrap();

        assert_eq!(value, 7);
    }

    #[test]
    fn direct_main_replies_accept_ok_and_preserve_failures() {
        assert_eq!(require_ok_main_reply("ok LauncherSuspended"), Ok(()));
        assert_eq!(
            require_ok_main_reply("rejected LauncherCrashed"),
            Err("rejected LauncherCrashed".to_string())
        );
        assert_eq!(
            require_ok_main_reply("error parse-failed"),
            Err("error parse-failed".to_string())
        );
    }

    #[test]
    fn control_request_validation_is_portable_and_authenticates_before_dispatch() {
        assert!(
            parse_control_request("{", "secret", false)
                .unwrap_err()
                .message
                .starts_with("invalid json:")
        );
        assert_eq!(
            parse_control_request(r#"{"id":7,"token":"wrong","cmd":"ping"}"#, "secret", false)
                .unwrap_err(),
            ControlRequestError {
                id: Some(json!(7)),
                message: "unauthorized".to_string(),
            }
        );
        assert_eq!(
            parse_control_request(r#"{"id":8,"token":"secret"}"#, "secret", false).unwrap_err(),
            ControlRequestError {
                id: Some(json!(8)),
                message: "missing cmd".to_string(),
            }
        );

        assert_eq!(
            parse_control_request(
                r#"{"id":9,"token":"secret","cmd":"status"}"#,
                "secret",
                false,
            )
            .unwrap(),
            ControlRequest {
                id: Some(json!(9)),
                cmd: "status".to_string(),
                args: json!({}),
            }
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn invalid_request_failure_retains_legacy_text_and_protocol_version() {
        let line = linux::failure_response(
            Some(json!(9)),
            "missing cmd",
            mister_magik_agent_protocol::FailureCode::InvalidRequest,
            mister_magik_agent_protocol::FailurePhase::Request,
            mister_magik_agent_protocol::RetryPolicy::Never,
            false,
        );
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["error"], "missing cmd");
        assert_eq!(value["failure"]["code"], "invalid_request");
        assert_eq!(value["failure"]["phase"], "request");
        assert_eq!(mister_magik_agent_protocol::PROTOCOL_VERSION, 2);

        let auth: Value =
            serde_json::from_str(&linux::authentication_failure_response(Some(json!(10)))).unwrap();
        assert_eq!(auth["error"], "unauthorized");
        assert_eq!(auth["failure"]["code"], "authentication_required");
        assert_eq!(auth["failure"]["phase"], "authentication");

        let busy: Value =
            serde_json::from_str(&linux::busy_failure_response(None, "agent is busy")).unwrap();
        assert_eq!(busy["error"], "agent is busy");
        assert_eq!(busy["failure"]["code"], "device_busy");
        assert_eq!(busy["failure"]["phase"], "availability");
        assert_eq!(busy["failure"]["retry_policy"], "retry");

        let unavailable: Value = serde_json::from_str(&linux::unavailable_failure_response(
            Some(json!(11)),
            "producer stream unavailable",
        ))
        .unwrap();
        assert_eq!(unavailable["error"], "producer stream unavailable");
        assert_eq!(unavailable["failure"]["code"], "device_unavailable");
        assert_eq!(unavailable["failure"]["phase"], "availability");
        assert_eq!(unavailable["failure"]["retry_policy"], "retry");

        let operation: Value = serde_json::from_str(&linux::operation_failure_response(
            Some(json!(12)),
            "capture failed",
        ))
        .unwrap();
        assert_eq!(operation["error"], "capture failed");
        assert_eq!(operation["failure"]["code"], "operation_failed");
        assert_eq!(operation["failure"]["phase"], "operation");
        assert_eq!(operation["failure"]["retry_policy"], "never");

        let artifact = crate::alpha_candidate::InstallFailure {
            kind: crate::alpha_candidate::InstallFailureKind::ArtifactMismatch,
            detail: "installed hash mismatch".to_string(),
        };
        let artifact: Value = serde_json::from_str(&linux::alpha_candidate_failure_response(
            Some(json!(13)),
            &artifact,
        ))
        .unwrap();
        assert_eq!(artifact["error"], "installed hash mismatch");
        assert_eq!(artifact["failure"]["code"], "artifact_mismatch");
        assert_eq!(artifact["failure"]["phase"], "artifact");
        assert_eq!(artifact["failure"]["retry_policy"], "reconcile_then_retry");

        let recovery = crate::alpha_candidate::InstallFailure {
            kind: crate::alpha_candidate::InstallFailureKind::RecoveryRequired,
            detail: "config restore failed".to_string(),
        };
        let recovery: Value = serde_json::from_str(&linux::alpha_candidate_failure_response(
            Some(json!(14)),
            &recovery,
        ))
        .unwrap();
        assert_eq!(recovery["error"], "config restore failed");
        assert_eq!(recovery["failure"]["code"], "recovery_required");
        assert_eq!(recovery["failure"]["phase"], "recovery");
        assert_eq!(recovery["failure"]["retry_policy"], "operator_required");
        assert_eq!(recovery["failure"]["recovery_required"], true);
    }

    #[test]
    fn explicitly_disabled_control_auth_still_requires_a_command() {
        let request =
            parse_control_request(r#"{"id":1,"cmd":"ping","args":{"x":2}}"#, "", true).unwrap();
        assert_eq!(request.args, json!({"x": 2}));
        assert_eq!(request.cmd, "ping");
        assert_eq!(
            parse_control_request(r#"{"id":1}"#, "", true)
                .unwrap_err()
                .message,
            "missing cmd"
        );
    }

    #[test]
    fn sd_relative_paths_normalize_and_reject_escapes() {
        assert_eq!(sd_browse::normalize_sd_relative_path("").unwrap(), "/");
        assert_eq!(
            sd_browse::normalize_sd_relative_path("///games//NES/./").unwrap(),
            "/games/NES"
        );
        assert_eq!(
            sd_browse::normalize_sd_relative_path("/_Arcade").unwrap(),
            "/_Arcade"
        );
        assert!(sd_browse::normalize_sd_relative_path("../secret").is_err());
        assert!(sd_browse::normalize_sd_relative_path("/games/../secret").is_err());
        assert!(sd_browse::normalize_sd_relative_path("/media/fat").is_err());
        assert!(sd_browse::normalize_sd_relative_path("/media/fat/games").is_err());
    }

    #[test]
    fn sd_host_path_stays_under_root_after_normalization() {
        let root = std::path::Path::new("/media/fat");
        assert_eq!(sd_browse::sd_host_path(root, "/"), root);
        assert_eq!(
            sd_browse::sd_host_path(root, "/games/NES"),
            std::path::PathBuf::from("/media/fat/games/NES")
        );
    }

    #[cfg(unix)]
    #[test]
    fn sd_item_access_rejects_symlinks_that_escape_root() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!(
            "mister-magik-agent-sd-symlink-{}",
            std::process::id()
        ));
        let root = base.join("sd");
        let outside = base.join("outside.txt");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&outside, b"secret").unwrap();
        symlink(&outside, root.join("escape.txt")).unwrap();

        let error = sd_browse::stat_item_at_root(&root, "/escape.txt")
            .expect_err("an SD path must not follow a symlink outside its root");

        assert!(error.contains("outside SD root"), "{error}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn sd_list_dir_sorts_and_classifies_entries() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-agent-sd-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("folder10")).unwrap();
        std::fs::create_dir_all(root.join("folder2")).unwrap();
        std::fs::create_dir_all(root.join("MyVision")).unwrap();
        std::fs::create_dir_all(root.join("mame")).unwrap();
        std::fs::create_dir_all(root.join("MegaDrive")).unwrap();
        std::fs::write(root.join("file10.rom"), b"0123456789").unwrap();
        std::fs::write(root.join("file2.rom"), b"12").unwrap();
        std::fs::write(root.join(".hidden"), b"h").unwrap();
        std::fs::write(root.join("readonly.txt"), b"r").unwrap();
        let mut readonly_permissions = std::fs::metadata(root.join("readonly.txt"))
            .unwrap()
            .permissions();
        readonly_permissions.set_readonly(true);
        std::fs::set_permissions(root.join("readonly.txt"), readonly_permissions).unwrap();

        let visible = sd_browse::list_dir_at_root(&root, "/", false).unwrap();
        let entries = visible["entries"].as_array().unwrap();

        let names = entries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "folder2",
                "folder10",
                "mame",
                "MegaDrive",
                "MyVision",
                "file2.rom",
                "file10.rom",
                "readonly.txt"
            ]
        );
        assert_eq!(visible["show_hidden"], false);
        assert_eq!(entries[0]["kind"], "directory");
        let file2 = entries
            .iter()
            .find(|entry| entry["name"] == "file2.rom")
            .unwrap();
        let readonly = entries
            .iter()
            .find(|entry| entry["name"] == "readonly.txt")
            .unwrap();
        assert_eq!(file2["size"], 2);
        assert_eq!(readonly["readonly"], true);

        let hidden = sd_browse::list_dir_at_root(&root, "/", true).unwrap();
        let hidden_entries = hidden["entries"].as_array().unwrap();
        let hidden_names = hidden_entries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            hidden_names,
            vec![
                "folder2",
                "folder10",
                "mame",
                "MegaDrive",
                "MyVision",
                ".hidden",
                "file2.rom",
                "file10.rom",
                "readonly.txt"
            ]
        );
        assert_eq!(hidden["show_hidden"], true);
        let hidden_entry = hidden_entries
            .iter()
            .find(|entry| entry["name"] == ".hidden")
            .unwrap();
        assert_eq!(hidden_entry["hidden"], true);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sd_list_dir_returns_expected_json_shape() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-agent-sd-json-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("games")).unwrap();
        std::fs::write(root.join("MiSTer.ini"), b"ini").unwrap();

        let result = sd_browse::list_dir_at_root(&root, "/", false).unwrap();
        assert_eq!(result["schema"], "mister-magik-sd-list-dir-v1");
        assert_eq!(result["path"], "/");
        assert_eq!(result["show_hidden"], false);
        assert_eq!(result["entries"][0]["name"], "games");
        assert_eq!(result["entries"][0]["kind"], "directory");
        assert_eq!(result["entries"][1]["name"], "MiSTer.ini");
        assert_eq!(result["entries"][1]["kind"], "file");

        let err = sd_browse::list_dir_at_root(&root, "/missing", false).unwrap_err();
        assert!(err.contains("read_dir /missing"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sd_list_dir_v2_returns_only_render_critical_entry_fields() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-agent-sd-json-v2-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("games")).unwrap();
        std::fs::write(root.join("MiSTer.ini"), b"ini").unwrap();
        std::fs::write(root.join(".hidden"), b"hidden").unwrap();

        let result = sd_browse::list_dir_fast_at_root(&root, "/", false).unwrap();
        assert_eq!(result["schema"], "mister-magik-sd-list-dir-v2");
        assert_eq!(result["path"], "/");
        assert_eq!(result["show_hidden"], false);
        let entries = result["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["name"], "games");
        assert_eq!(entries[0]["kind"], "directory");
        assert_eq!(entries[1]["name"], "MiSTer.ini");
        assert_eq!(entries[1]["kind"], "file");
        for entry in entries {
            let object = entry.as_object().unwrap();
            assert_eq!(object.len(), 3);
            assert!(object.contains_key("name"));
            assert!(object.contains_key("path"));
            assert!(object.contains_key("kind"));
        }

        let hidden = sd_browse::list_dir_fast_at_root(&root, "/", true).unwrap();
        assert_eq!(hidden["entries"].as_array().unwrap().len(), 3);
        assert!(sd_browse::list_dir_fast_at_root(&root, "/missing", false).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sd_stat_item_reports_capabilities() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-agent-sd-stat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("_Arcade")).unwrap();
        std::fs::write(root.join("_Arcade/game.mra"), b"<misterromdescription/>").unwrap();

        let stat = sd_browse::stat_item_at_root(&root, "/_Arcade/game.mra").unwrap();

        assert_eq!(stat["schema"], "mister-magik-sd-stat-item-v1");
        assert_eq!(stat["extension"], "mra");
        assert_eq!(stat["capabilities"]["mra_parse"], true);
        assert_eq!(stat["capabilities"]["image_preview"], false);
        assert!(sd_browse::stat_item_at_root(&root, "/../escape").is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn trimmed_status_text_omits_blank_runtime_markers() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-agent-status-text-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&root);
        std::fs::write(&root, "  ArcadeCore\n").unwrap();
        assert_eq!(
            linux::read_trimmed_text_value(root.to_str().unwrap()),
            "ArcadeCore"
        );
        std::fs::write(&root, " \n").unwrap();
        assert_eq!(
            linux::read_trimmed_text_value(root.to_str().unwrap()),
            Value::Null
        );
        let _ = std::fs::remove_file(root);
    }

    #[test]
    fn sd_parse_mra_extracts_all_rows_and_raw_xml() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-agent-sd-mra-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("_Arcade")).unwrap();
        let xml = r#"<misterromdescription>
  <name>Moon Patrol</name>
  <setname>mpatrol</setname>
  <rbf>_Arcade/MoonPatrol.rbf</rbf>
  <buttons names="Start,Fire,Jump" default="A,B"/>
  <rom index="0" zip="mpatrol.zip">roms/mpatrol.rom</rom>
</misterromdescription>"#;
        std::fs::write(root.join("_Arcade/Moon Patrol.mra"), xml).unwrap();

        let parsed = sd_browse::parse_mra_at_root(&root, "/_Arcade/Moon Patrol.mra").unwrap();
        let rows = parsed["xml_rows"].as_array().unwrap();
        let path_rows = parsed["path_rows"].as_array().unwrap();

        assert_eq!(parsed["schema"], "mister-magik-sd-parse-mra-v1");
        assert!(parsed["raw_xml"].as_str().unwrap().contains("Moon Patrol"));
        assert!(
            rows.iter()
                .any(|row| row["kind"] == "attribute" && row["name"] == "@zip")
        );
        assert!(
            rows.iter()
                .any(|row| row["kind"] == "text" && row["value"] == "Moon Patrol")
        );
        assert!(
            path_rows
                .iter()
                .any(|row| row["value"].as_str().unwrap_or("").contains("mpatrol.zip"))
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sd_preview_image_reports_png_dimensions_and_payload() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-agent-sd-png-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("images")).unwrap();
        let png = [
            0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 13, b'I', b'H', b'D', b'R',
            0, 0, 0, 2, 0, 0, 0, 3, 8, 6, 0, 0, 0,
        ];
        std::fs::write(root.join("images/shot.png"), png).unwrap();

        let preview = sd_browse::preview_image_at_root(&root, "/images/shot.png").unwrap();

        assert_eq!(preview.result["schema"], "mister-magik-sd-preview-image-v1");
        assert_eq!(preview.result["format"], "png");
        assert_eq!(preview.result["width"], 2);
        assert_eq!(preview.result["height"], 3);
        assert_eq!(
            preview.result["payload_bytes"],
            preview.payload.len() as u64
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sd_image_dimensions_reject_invalid_png_header_and_zero_geometry() {
        let mut wrong_chunk = [0u8; 24];
        wrong_chunk[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        wrong_chunk[12..16].copy_from_slice(b"NOPE");
        wrong_chunk[16..20].copy_from_slice(&2u32.to_be_bytes());
        wrong_chunk[20..24].copy_from_slice(&3u32.to_be_bytes());
        assert_eq!(sd_browse::image_dimensions(&wrong_chunk), None);

        let mut zero_width = wrong_chunk;
        zero_width[12..16].copy_from_slice(b"IHDR");
        zero_width[16..20].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(sd_browse::image_dimensions(&zero_width), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn telemetry_cpu_delta_math_reports_busy_percent() {
        let previous = linux::CpuTimes {
            user: 100,
            nice: 0,
            system: 100,
            idle: 800,
            iowait: 0,
            irq: 0,
            softirq: 0,
            steal: 0,
        };
        let current = linux::CpuTimes {
            user: 150,
            nice: 0,
            system: 150,
            idle: 900,
            iowait: 0,
            irq: 0,
            softirq: 0,
            steal: 0,
        };
        assert_eq!(linux::cpu_busy_percent(previous, current), 50.0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn telemetry_phase_aggregation_uses_fixture_counts_without_device_reads() {
        let evidence = linux::aggregate_process_telemetry_evidence(&[
            linux::ProcessTelemetryEvidence {
                discovery_us: 10,
                proc_parse_us: 20,
                child_processes: 1,
                files_read: 4,
            },
            linux::ProcessTelemetryEvidence {
                discovery_us: 30,
                proc_parse_us: 40,
                child_processes: 2,
                files_read: 6,
            },
        ]);
        assert_eq!(evidence.discovery_us, 40);
        assert_eq!(evidence.proc_parse_us, 60);
        assert_eq!(evidence.child_processes, 3);
        assert_eq!(evidence.files_read, 10);
        assert_eq!(
            linux::telemetry_deadline_overrun_us(
                Duration::from_millis(101),
                Duration::from_millis(100)
            ),
            1_000
        );
        assert_eq!(
            linux::telemetry_deadline_overrun_us(
                Duration::from_millis(99),
                Duration::from_millis(100)
            ),
            0
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn telemetry_cpu_delta_math_treats_iowait_as_idle() {
        let previous = linux::CpuTimes {
            user: 100,
            nice: 0,
            system: 100,
            idle: 800,
            iowait: 0,
            irq: 0,
            softirq: 0,
            steal: 0,
        };
        let wait_only = linux::CpuTimes {
            user: 100,
            nice: 0,
            system: 100,
            idle: 800,
            iowait: 100,
            irq: 0,
            softirq: 0,
            steal: 0,
        };
        let mixed = linux::CpuTimes {
            user: 125,
            nice: 0,
            system: 125,
            idle: 800,
            iowait: 150,
            irq: 0,
            softirq: 0,
            steal: 0,
        };

        assert_eq!(linux::cpu_busy_percent(previous, wait_only), 0.0);
        assert_eq!(linux::cpu_busy_percent(previous, mixed), 25.0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn telemetry_cpu_parser_keeps_aggregate_and_cores() {
        let rows = linux::parse_cpu_times_text(
            "cpu  1 2 3 4 5 6 7 8 9 10\ncpu0 10 0 20 70 0 0 0 0\ncpu1 20 0 10 70 0 0 0 0\nintr 1 2 3\n",
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].user, 1);
        assert_eq!(rows[1].system, 20);
        assert_eq!(rows[2].idle, 70);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn telemetry_resolves_sd_backing_disk_and_fallback() {
        let stats = "179 0 mmcblk0 1 0 2 0 3 0 4 0 0 0 0 0 0 0\n179 1 mmcblk0p1 1 0 2 0 3 0 4 0 0 0 0 0 0 0\n";
        assert_eq!(
            linux::parse_backing_disk(
                "/dev/mmcblk0p1 /media/fat fuseblk rw 0 0\n",
                stats,
                "/media/fat"
            ),
            Some("mmcblk0".to_string())
        );
        assert_eq!(
            linux::parse_backing_disk("root /media/fat fuseblk rw 0 0\n", stats, "/media/fat"),
            Some("mmcblk0".to_string())
        );
        let ambiguous = format!("{stats}179 8 mmcblk1 1 0 2 0 3 0 4 0 0 0 0 0 0 0\n");
        assert_eq!(
            linux::parse_backing_disk("root /media/fat fuseblk rw 0 0\n", &ambiguous, "/media/fat"),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn telemetry_parses_diskstats_and_calculates_rates() {
        let stats = "179 0 mmcblk0 10 2 100 4 20 6 200 8 0 10 12 0 0 0 0\n";
        let counters = linux::parse_disk_counters(stats, "mmcblk0").unwrap();
        assert_eq!(counters.sectors_read, 100);
        assert_eq!(counters.sectors_written, 200);
        let previous = linux::DiskCounters {
            sectors_read: 80,
            sectors_written: 160,
        };
        assert_eq!(
            linux::disk_rate_bytes_per_sec(previous, counters, std::time::Duration::from_secs(2)),
            Some((5_120, 10_240))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn telemetry_disk_rate_rejects_reset_and_percent_clamps() {
        let previous = linux::DiskCounters {
            sectors_read: 100,
            sectors_written: 200,
        };
        let reset = linux::DiskCounters {
            sectors_read: 99,
            sectors_written: 201,
        };
        assert_eq!(
            linux::disk_rate_bytes_per_sec(previous, reset, std::time::Duration::from_secs(1)),
            None
        );
        assert_eq!(linux::throughput_percent(25_000_000, 50_000_000), 50.0);
        assert_eq!(linux::throughput_percent(75_000_000, 50_000_000), 100.0);
        assert_eq!(linux::throughput_percent(1, 0), 0.0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn telemetry_processor_parser_handles_a_parenthesized_process_name() {
        let fields = (0..37)
            .map(|index| {
                if index == 0 {
                    "S".to_string()
                } else {
                    index.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            linux::parse_proc_stat_processor(&format!("42 (mister magik) {fields}")),
            Some(36)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn telemetry_process_discovery_scans_once_and_reads_each_matching_status_once() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-agent-proc-fixture-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for (pid, name, status) in [
            (31, "mister-magik-fb", Some("VmRSS:\t120 kB\nThreads:\t3\n")),
            (7, "mister-magik-fb", Some("Threads:\t2\nVmRSS:\t80 kB\n")),
            (19, "MiSTer_MagiKDev", Some("VmRSS:\tbad\nThreads:\t4\n")),
            (11, "MiSTer_MagiK", None),
            (55, "unrelated", Some("VmRSS:\t999 kB\nThreads:\t99\n")),
        ] {
            let process = root.join(pid.to_string());
            std::fs::create_dir_all(&process).unwrap();
            std::fs::write(process.join("comm"), format!("{name}\n")).unwrap();
            if let Some(status) = status {
                std::fs::write(process.join("status"), status).unwrap();
            }
        }
        std::fs::create_dir_all(root.join("not-a-pid")).unwrap();

        let snapshot = linux::process_telemetry_snapshot_at(&root);

        assert_eq!(
            snapshot.processes["mister-magik-fb"]["pids"],
            json!([7, 31])
        );
        assert_eq!(snapshot.processes["mister-magik-fb"]["rss_kb"], 200);
        assert_eq!(snapshot.processes["mister-magik-fb"]["threads"], 5);
        assert_eq!(snapshot.processes["MiSTer_MagiKDev"]["rss_kb"], 0);
        assert_eq!(snapshot.processes["MiSTer_MagiKDev"]["threads"], 4);
        assert_eq!(snapshot.processes["MiSTer_MagiK"]["pids"], json!([11]));
        assert_eq!(snapshot.processes["MiSTer_MagiK"]["rss_kb"], 0);
        assert_eq!(snapshot.evidence.child_processes, 0);
        assert_eq!(snapshot.evidence.files_read, 8);

        let malformed = linux::parse_process_status(
            "VmRSS:\t4096 kB\nThreads:\tbroken\nThreads:\t7\nOther: 9\n",
        );
        assert_eq!(malformed.rss_kb, 4096);
        assert_eq!(malformed.threads, 7);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn telemetry_ui_pid_prefers_the_current_status_process() {
        let pids = serde_json::json!([11, 22]);
        assert_eq!(
            linux::launcher_ui_pid(&serde_json::json!({"pid": 22}), &pids),
            Some(22)
        );
        assert_eq!(
            linux::launcher_ui_pid(&serde_json::json!({"pid": 33}), &pids),
            None
        );
        assert_eq!(
            linux::launcher_ui_pid(&serde_json::json!({"pid": 33}), &serde_json::json!([11])),
            Some(11)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn telemetry_memory_split_balances_total() {
        let value = linux::memory_split_json(1_000, 300, 100, 20);
        assert_eq!(value["magik_kb"], 100);
        assert_eq!(value["other_used_kb"], 600);
        assert_eq!(value["available_kb"], 300);
        assert_eq!(
            value["magik_kb"].as_u64().unwrap()
                + value["other_used_kb"].as_u64().unwrap()
                + value["available_kb"].as_u64().unwrap(),
            1_000
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn telemetry_network_rates_use_elapsed_delta() {
        let start = std::time::Instant::now();
        let previous = linux::NetSample {
            rx_bytes: 1_000,
            tx_bytes: 2_000,
            at: Some(start),
        };
        let current = linux::NetSample {
            rx_bytes: 2_000,
            tx_bytes: 2_500,
            at: Some(start + std::time::Duration::from_secs(2)),
        };
        let value = linux::network_rate_json(Some(previous), current);
        assert_eq!(value["rx_bytes_per_sec"], 500);
        assert_eq!(value["tx_bytes_per_sec"], 250);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn video_diagnostics_distinguishes_changed_and_unverified_ownership() {
        assert_eq!(
            linux::video_diagnostics_ownership_state(Some(4), Some(4), Some(true), true),
            (false, false)
        );
        assert_eq!(
            linux::video_diagnostics_ownership_state(Some(4), Some(5), Some(false), false),
            (true, false)
        );
        assert_eq!(
            linux::video_diagnostics_ownership_state(Some(4), Some(4), None, true),
            (false, true)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn video_diagnostics_rejects_captured_output_trigger_eight_provenance() {
        use mister_magik_video_diagnostics_contract as contract;
        assert_eq!(
            linux::video_diagnostics_trigger_provenance(
                contract::VIDEO_DIAGNOSTICS_TRIGGER_AVALON_TIMEOUT,
                contract::VIDEO_DIAGNOSTICS_TRIGGER_NONE,
                contract::VIDEO_DIAGNOSTICS_TRIGGER_AVALON_TIMEOUT,
            ),
            (true, true, false)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hdmi_lock_evidence_classification_prioritizes_sticky_loss() {
        use mister_magik_video_diagnostics_contract as contract;
        assert_eq!(linux::hdmi_lock_classification(0), "hdmi_pll_not_seen");
        assert_eq!(
            linux::hdmi_lock_classification(contract::HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH),
            "hdmi_pll_not_stably_armed"
        );
        let locked = contract::HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH
            | contract::HDMI_EVIDENCE_FLAG_LOCK_ARMED
            | contract::HDMI_EVIDENCE_FLAG_LOCK_CURRENT;
        assert_eq!(linux::hdmi_lock_classification(locked), "hdmi_pll_locked");
        assert_eq!(
            linux::hdmi_lock_classification(locked | contract::HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST),
            "hdmi_pll_lock_lost"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hdmi_lock_evidence_ack_and_fallback_policy_are_strict() {
        use linux::{HdmiEvidenceAck, HdmiEvidenceProbeAction};
        use mister_magik_video_diagnostics_contract as contract;
        assert_eq!(
            linux::classify_hdmi_evidence_ack(contract::HDMI_EVIDENCE_MAGIC, 0),
            HdmiEvidenceAck::Present
        );
        assert_eq!(
            linux::classify_hdmi_evidence_ack(0, contract::HDMI_EVIDENCE_MAGIC),
            HdmiEvidenceAck::Present
        );
        assert_eq!(
            linux::classify_hdmi_evidence_ack(0, 0),
            HdmiEvidenceAck::Unsupported
        );
        assert_eq!(
            linux::classify_hdmi_evidence_ack(0x1234, 0x5678),
            HdmiEvidenceAck::Invalid
        );
        assert_eq!(
            linux::classify_hdmi_evidence_ack(0, 0x5678),
            HdmiEvidenceAck::Invalid
        );

        assert_eq!(
            linux::hdmi_evidence_probe_action(None, false),
            HdmiEvidenceProbeAction::UseNew
        );
        assert_eq!(
            linux::hdmi_evidence_probe_action(Some(std::io::ErrorKind::NotFound), false),
            HdmiEvidenceProbeAction::ReadLegacy
        );
        for kind in [
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::TimedOut,
        ] {
            assert_eq!(
                linux::hdmi_evidence_probe_action(Some(kind), false),
                HdmiEvidenceProbeAction::RetryNew
            );
            assert_eq!(
                linux::hdmi_evidence_probe_action(Some(kind), true),
                HdmiEvidenceProbeAction::Fail
            );
        }
        assert_eq!(
            linux::hdmi_evidence_probe_action(Some(std::io::ErrorKind::NotFound), true),
            HdmiEvidenceProbeAction::Fail
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hdmi_output_activity_ack_and_modular_classification_are_strict() {
        use linux::{HdmiEvidenceAck, HdmiOutputActivityDeltas};
        use mister_magik_video_diagnostics_contract as contract;

        assert_eq!(
            linux::classify_hdmi_output_activity_ack(contract::HDMI_OUTPUT_ACTIVITY_MAGIC, 0,),
            HdmiEvidenceAck::Present
        );
        assert_eq!(
            linux::classify_hdmi_output_activity_ack(0, 0),
            HdmiEvidenceAck::Unsupported
        );
        assert_eq!(
            linux::classify_hdmi_output_activity_ack(0x1234, 0),
            HdmiEvidenceAck::Invalid
        );

        let wrapped = linux::hdmi_output_activity_deltas(14, 3, 7, 1, 3, 9);
        assert_eq!(
            wrapped,
            HdmiOutputActivityDeltas {
                no_de: 3,
                de_all_zero: 0,
                de_has_nonzero: 2,
            }
        );
        assert_eq!(
            linux::hdmi_output_activity_classification(
                contract::HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
                HdmiOutputActivityDeltas {
                    no_de: 0,
                    de_all_zero: 0,
                    de_has_nonzero: 0,
                },
            ),
            "final_output_no_completed_frame"
        );
        for (deltas, expected) in [
            (
                HdmiOutputActivityDeltas {
                    no_de: 2,
                    de_all_zero: 0,
                    de_has_nonzero: 0,
                },
                "final_output_no_de",
            ),
            (
                HdmiOutputActivityDeltas {
                    no_de: 0,
                    de_all_zero: 2,
                    de_has_nonzero: 0,
                },
                "final_output_de_all_zero",
            ),
            (
                HdmiOutputActivityDeltas {
                    no_de: 0,
                    de_all_zero: 0,
                    de_has_nonzero: 2,
                },
                "final_output_de_has_nonzero",
            ),
            (
                HdmiOutputActivityDeltas {
                    no_de: 1,
                    de_all_zero: 0,
                    de_has_nonzero: 1,
                },
                "final_output_mixed",
            ),
        ] {
            assert_eq!(
                linux::hdmi_output_activity_classification(
                    contract::HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
                    deltas,
                ),
                expected
            );
        }
        assert_eq!(
            linux::hdmi_output_activity_classification(
                contract::HDMI_OUTPUT_ACTIVITY_FLAG_COUNTER_COLLISION,
                HdmiOutputActivityDeltas {
                    no_de: 0,
                    de_all_zero: 0,
                    de_has_nonzero: 1,
                },
            ),
            "final_output_activity_invalid"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detailed_video_path_classification_is_conservative_and_decisive() {
        use linux::{AvalonLivenessDeltas, FinalPathDeltas, ThreeClassDeltas};
        use mister_magik_video_diagnostics_contract as contract;

        let scaled_black = FinalPathDeltas {
            no_de: 0,
            black_direct: 0,
            black_scaled: 3,
            black_mixed: 0,
            nonzero: 0,
        };
        let raw_black = ThreeClassDeltas {
            no_de: 0,
            all_zero: 3,
            nonzero: 0,
        };
        let post_black = raw_black;
        let avalon_live = AvalonLivenessDeltas {
            bucket: 9,
            request: 8,
            accepted: 8,
            returned: 8,
        };
        assert_eq!(
            linux::detailed_path_classification(
                linux::PathActivityFlags {
                    avalon: contract::HDMI_AVALON_LIVENESS_ACTIVITY_FLAG_BUCKET_VALID,
                    ..Default::default()
                },
                scaled_black,
                raw_black,
                post_black,
                avalon_live,
            ),
            "scaler_raw_black_with_framebuffer_returns"
        );

        let raw_nonzero = ThreeClassDeltas {
            no_de: 0,
            all_zero: 0,
            nonzero: 3,
        };
        assert_eq!(
            linux::detailed_path_classification(
                linux::PathActivityFlags::default(),
                scaled_black,
                raw_nonzero,
                post_black,
                avalon_live,
            ),
            "post_scaler_processing_blacked_video"
        );

        let direct_black = FinalPathDeltas {
            black_direct: 3,
            black_scaled: 0,
            ..scaled_black
        };
        assert_eq!(
            linux::detailed_path_classification(
                linux::PathActivityFlags::default(),
                direct_black,
                raw_black,
                post_black,
                avalon_live,
            ),
            "final_output_black_direct_path"
        );
        assert_eq!(
            linux::detailed_path_classification(
                linux::PathActivityFlags {
                    final_path: contract::HDMI_FINAL_PATH_ACTIVITY_FLAG_COUNTER_COLLISION,
                    ..Default::default()
                },
                scaled_black,
                raw_black,
                post_black,
                avalon_live,
            ),
            "video_path_evidence_invalid"
        );

        assert_eq!(
            linux::detailed_path_classification(
                linux::PathActivityFlags {
                    avalon: contract::HDMI_AVALON_LIVENESS_ACTIVITY_FLAG_BUCKET_VALID,
                    ..Default::default()
                },
                scaled_black,
                raw_black,
                post_black,
                AvalonLivenessDeltas {
                    bucket: 0,
                    request: 0,
                    accepted: 0,
                    returned: 0,
                },
            ),
            "scaler_raw_black_no_observed_avalon_bucket"
        );

        assert_eq!(
            linux::detailed_path_classification(
                linux::PathActivityFlags {
                    avalon: contract::HDMI_AVALON_LIVENESS_ACTIVITY_FLAG_BUCKET_VALID,
                    ..Default::default()
                },
                scaled_black,
                ThreeClassDeltas {
                    no_de: 0,
                    all_zero: 1,
                    nonzero: 0,
                },
                post_black,
                avalon_live,
            ),
            "final_output_black_scaled_path_inconclusive"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn scaler_fetch_classification_is_observational_and_strict() {
        use linux::ScalerFetchDeltas;
        use mister_magik_video_diagnostics_contract as contract;

        let stalled = ScalerFetchDeltas {
            batch_two: 0,
            starved_frame: 2,
        };
        assert_eq!(
            linux::scaler_fetch_classification(
                contract::HDMI_SCALER_FETCH_ACTIVITY_FLAG_SNAPSHOT_VALID,
                stalled,
            ),
            "scaler_fetch_stalled_with_two_reads"
        );
        assert_eq!(
            linux::scaler_fetch_classification(
                contract::HDMI_SCALER_FETCH_ACTIVITY_FLAG_SNAPSHOT_VALID
                    | contract::HDMI_SCALER_FETCH_ACTIVITY_FLAG_COMPLETION_DELTA_INVALID,
                stalled,
            ),
            "scaler_fetch_evidence_invalid"
        );
        assert_eq!(
            linux::scaler_fetch_classification(
                contract::HDMI_SCALER_FETCH_ACTIVITY_FLAG_SNAPSHOT_VALID,
                ScalerFetchDeltas {
                    batch_two: 1,
                    starved_frame: 0,
                },
            ),
            "scaler_fetch_recovered_two_completion_batch"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ordered_signature_classification_requires_three_advancing_frames() {
        use mister_magik_video_diagnostics_contract as contract;

        let sample = |sequence: u16, signature: u16| {
            let mut words = [0; contract::RAW_SCALER_ROLLBACK_ORDERED_SIGNATURE_V3_WORDS];
            words[contract::RAW_SCALER_ROLLBACK_ORDERED_SIGNATURE_V3_SCHEMA_WORD] =
                contract::RAW_SCALER_ROLLBACK_ORDERED_SIGNATURE_V3_SCHEMA;
            words[contract::RAW_SCALER_ROLLBACK_ORDERED_SIGNATURE_V3_FLAGS_WORD] =
                contract::RAW_SCALER_ROLLBACK_ORDERED_SIGNATURE_V3_FLAG_FRAME_VALID;
            words[contract::RAW_SCALER_ROLLBACK_ORDERED_SIGNATURE_V3_FRAME_SEQUENCE_WORD] =
                sequence;
            words[contract::RAW_SCALER_ROLLBACK_ORDERED_SIGNATURE_V3_ORDERED_SIGNATURE_WORD] =
                signature;
            contract::RawScalerOrderedSignatureState { words }
        };
        let stable = [
            sample(100, 0x5678),
            sample(101, 0x5678),
            sample(103, 0x5678),
        ];
        assert_eq!(
            linux::raw_scaler_classification([&stable[0], &stable[1], &stable[2]]),
            "raw_scaler_ordered_stable"
        );

        let varying = [
            sample(100, 0x5678),
            sample(101, 0x5679),
            sample(103, 0x5678),
        ];
        assert_eq!(
            linux::raw_scaler_classification([&varying[0], &varying[1], &varying[2]]),
            "raw_scaler_order_changed_requires_static_source_proof"
        );

        let nonadvancing = [stable[0].clone(), stable[0].clone(), stable[2].clone()];
        assert_eq!(
            linux::raw_scaler_classification([
                &nonadvancing[0],
                &nonadvancing[1],
                &nonadvancing[2],
            ]),
            "raw_scaler_ordered_evidence_inconclusive"
        );

        let wrapping = [
            sample(0xfffe, 0x5678),
            sample(0xffff, 0x5678),
            sample(0x0001, 0x5678),
        ];
        assert_eq!(
            linux::raw_scaler_classification([&wrapping[0], &wrapping[1], &wrapping[2]]),
            "raw_scaler_ordered_stable"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn scaler_fetch_liveness_classification_is_temporal_and_observational() {
        use mister_magik_video_diagnostics_contract as contract;

        let sample = |sequence: u8, frozen_address_fold: u8, flags: u16, cause: u16| {
            let mut words = [0; contract::SCALER_FETCH_LIVENESS_STATE_WORDS];
            words[contract::SCALER_FETCH_LIVENESS_STATE_SCHEMA_WORD] =
                contract::SCALER_FETCH_LIVENESS_STATE_SCHEMA;
            words[contract::SCALER_FETCH_LIVENESS_STATE_FLAGS_WORD] = flags
                | (u16::from(sequence)
                    << contract::SCALER_FETCH_LIVENESS_STATE_PUBLICATION_SEQUENCE_BIT);
            words[contract::SCALER_FETCH_LIVENESS_STATE_STATE_WORD] = cause
                | (u16::from(frozen_address_fold)
                    << contract::SCALER_FETCH_LIVENESS_STATE_FROZEN_ADDRESS_FOLD_BIT);
            contract::ScalerFetchLivenessState { words }
        };
        let valid = contract::SCALER_FETCH_LIVENESS_STATE_FLAG_RECORD_VALID;
        let normal = valid | contract::SCALER_FETCH_LIVENESS_STATE_FLAG_NORMAL_LIVENESS_SEEN;
        let normal_samples = [
            sample(14, 0, normal, 0),
            sample(15, 0, normal, 0),
            sample(0, 0, normal, 0),
        ];
        assert_eq!(
            linux::scaler_fetch_liveness_classification([
                &normal_samples[0],
                &normal_samples[1],
                &normal_samples[2],
            ]),
            "scaler_fetch_normal_liveness"
        );

        let skipped_samples = [
            sample(14, 0, normal, 0),
            sample(15, 0, normal, 0),
            sample(1, 0, normal, 0),
        ];
        assert_eq!(
            linux::scaler_fetch_liveness_classification([
                &skipped_samples[0],
                &skipped_samples[1],
                &skipped_samples[2],
            ]),
            "scaler_fetch_liveness_evidence_inconclusive"
        );

        for (cause, expected) in [
            (
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_NO_REQUEST_SEEN,
                "scaler_fetch_no_request_seen",
            ),
            (
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_ACCEPT_BLOCKED,
                "scaler_fetch_accept_blocked",
            ),
            (
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_FIRST_RETURN_MISSING,
                "scaler_fetch_first_return_missing",
            ),
            (
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_RETURN_INCOMPLETE,
                "scaler_fetch_return_incomplete",
            ),
            (
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_REQUEST_CANCELLED,
                "scaler_fetch_request_cancelled",
            ),
        ] {
            let flags = valid | contract::SCALER_FETCH_LIVENESS_STATE_FLAG_FIRST_STALL_VALID;
            let samples = [
                sample(10, 7, flags, cause),
                sample(11, 7, flags, cause),
                sample(12, 7, flags, cause),
            ];
            assert_eq!(
                linux::scaler_fetch_liveness_classification([
                    &samples[0],
                    &samples[1],
                    &samples[2],
                ]),
                expected
            );
        }

        let changed_frozen_tuple = [
            sample(
                4,
                7,
                valid | contract::SCALER_FETCH_LIVENESS_STATE_FLAG_FIRST_STALL_VALID,
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_RETURN_INCOMPLETE,
            ),
            sample(
                5,
                8,
                valid | contract::SCALER_FETCH_LIVENESS_STATE_FLAG_FIRST_STALL_VALID,
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_RETURN_INCOMPLETE,
            ),
            sample(
                6,
                7,
                valid | contract::SCALER_FETCH_LIVENESS_STATE_FLAG_FIRST_STALL_VALID,
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_RETURN_INCOMPLETE,
            ),
        ];
        assert_eq!(
            linux::scaler_fetch_liveness_classification([
                &changed_frozen_tuple[0],
                &changed_frozen_tuple[1],
                &changed_frozen_tuple[2],
            ]),
            "scaler_fetch_liveness_evidence_inconclusive"
        );

        let transitioning = [
            sample(10, 0, valid, 0),
            sample(
                11,
                10,
                valid | contract::SCALER_FETCH_LIVENESS_STATE_FLAG_FIRST_STALL_VALID,
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_NO_REQUEST_SEEN,
            ),
            sample(
                12,
                10,
                valid | contract::SCALER_FETCH_LIVENESS_STATE_FLAG_FIRST_STALL_VALID,
                contract::SCALER_FETCH_LIVENESS_STATE_CAUSE_NO_REQUEST_SEEN,
            ),
        ];
        assert_eq!(
            linux::scaler_fetch_liveness_classification([
                &transitioning[0],
                &transitioning[1],
                &transitioning[2],
            ]),
            "scaler_fetch_liveness_evidence_inconclusive"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn scaler_fetch_ordered_classification_is_fail_closed() {
        use mister_magik_video_diagnostics_contract as contract;

        let sample = |sequence: u16, signature: u16, flags: u16| {
            let mut words = [0; contract::RAW_SCALER_STATE_WORDS];
            words[contract::RAW_SCALER_STATE_SCHEMA_WORD] = contract::RAW_SCALER_STATE_SCHEMA;
            words[contract::RAW_SCALER_STATE_FLAGS_WORD] = flags;
            words[contract::RAW_SCALER_STATE_CAPTURE_SEQUENCE_WORD] = sequence;
            words[contract::RAW_SCALER_STATE_ORDERED_SIGNATURE_WORD] = signature;
            contract::ScalerFetchOrderedSignatureState { words }
        };
        let valid = contract::RAW_SCALER_STATE_FLAG_CAPTURE_VALID;
        let stable = [
            sample(100, 0x5678, valid),
            sample(101, 0x5678, valid),
            sample(103, 0x5678, valid),
        ];
        assert_eq!(
            linux::scaler_fetch_ordered_classification([&stable[0], &stable[1], &stable[2]]),
            "scaler_fetch_ordered_stable"
        );
        let changed = [
            stable[0].clone(),
            sample(101, 0x5679, valid),
            stable[2].clone(),
        ];
        assert_eq!(
            linux::scaler_fetch_ordered_classification([&changed[0], &changed[1], &changed[2],]),
            "scaler_fetch_order_changed_requires_static_source_proof"
        );
        let fault = [
            stable[0].clone(),
            sample(0, 0, contract::RAW_SCALER_STATE_FLAG_UNEXPECTED_RETURN),
            stable[2].clone(),
        ];
        assert_eq!(
            linux::scaler_fetch_ordered_classification([&fault[0], &fault[1], &fault[2]]),
            "scaler_fetch_ordered_evidence_inconclusive"
        );
    }

    #[test]
    fn io_operation_evidence_uses_common_monotonic_and_capacity_fields() {
        let evidence = io_operation_evidence(IoOperationEvidence {
            operation: "fixture_operation",
            request_received_monotonic_us: 900,
            operation_started_monotonic_us: 1_000,
            operation_ended_monotonic_us: 1_250,
            bytes_read: 4_096,
            bytes_written: 1_024,
            peak_buffer_ownership_bytes: 5_120,
            peak_rss_kb: Some(12_345),
            phases_us: serde_json::json!({"read": 100, "serialization": 50}),
        });
        assert_eq!(evidence["schema"], "mister-magik-agent-io-operation-v1");
        assert_eq!(evidence["clock_domain"], "CLOCK_MONOTONIC");
        assert_eq!(evidence["elapsed_us"], 250);
        assert_eq!(evidence["bytes_read"], 4_096);
        assert_eq!(evidence["bytes_written"], 1_024);
        assert_eq!(evidence["peak_buffer_ownership_bytes"], 5_120);
        assert_eq!(evidence["peak_rss_kb"], 12_345);
        assert_eq!(evidence["phases_us"]["serialization"], 50);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_agent_reports_platform_error() {
        let err = linux::run(&[]).unwrap_err().to_string();
        assert!(err.contains("only run on Linux/MiSTer"));
    }
}
