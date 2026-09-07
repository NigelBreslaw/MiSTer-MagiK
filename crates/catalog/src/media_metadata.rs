// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Media metadata parsing helpers.

#[cfg(any(test, feature = "builder"))]
use crate::catalog_scan::FoundFile;
#[cfg(any(test, feature = "builder"))]
use crate::launch_profiles::CollectionListing;
#[cfg(test)]
const MRA_PREFIX_BYTES: usize = 160 * 1024;
use quick_xml::Reader as XmlReader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
#[cfg(target_os = "linux")]
#[cfg(any(test, feature = "builder"))]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "builder"))]
use std::process::{Command, Stdio};
#[cfg(any(test, feature = "builder"))]
use std::sync::mpsc;
#[cfg(any(test, feature = "builder"))]
use std::time::{Duration, Instant};

const MGL_PREFIX_BYTES: usize = 32 * 1024;
#[cfg(any(test, feature = "builder"))]
const MAX_COLLECTION_LISTING_BYTES: usize = 8 * 1024 * 1024;

#[cfg(any(test, feature = "builder"))]
pub(crate) fn collection_listing_text_with_tool_result(
    file: &FoundFile,
    listing: &CollectionListing,
    tool: &Path,
    timeout: Duration,
) -> Result<Option<String>, String> {
    let mut command = Command::new(tool);
    command
        .args(["e", "-so"])
        .arg(&file.path)
        .arg(&listing.entry_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(target_os = "linux")]
    unsafe {
        command.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("start archive listing helper: {error}"))?;
    let start = Instant::now();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "archive listing helper has no output pipe".to_string())?;
    let (output_tx, output_rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut limited = stdout.take((MAX_COLLECTION_LISTING_BYTES + 1) as u64);
        let result = limited.read_to_end(&mut output).map(|_| {
            let within_limit = result_is_within_limit(output.len());
            (output, within_limit)
        });
        let _ = output_tx.send(result);
    });
    let output = loop {
        if let Ok(result) = output_rx.try_recv() {
            let (output, within_limit) = match result {
                Ok(result) => result,
                Err(error) => {
                    terminate_archive_helper(&mut child);
                    return Err(format!("read archive listing: {error}"));
                }
            };
            if !within_limit {
                terminate_archive_helper(&mut child);
                return Err(format!(
                    "archive listing exceeds {} bytes",
                    MAX_COLLECTION_LISTING_BYTES
                ));
            }
            break output;
        }
        if start.elapsed() >= timeout {
            terminate_archive_helper(&mut child);
            return Err(format!(
                "archive listing helper exceeded {} ms",
                timeout.as_millis()
            ));
        }
        if child
            .try_wait()
            .map_err(|error| format!("poll archive listing helper: {error}"))?
            .is_some()
        {
            let (output, within_limit) = match output_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => return Err(format!("read archive listing: {error}")),
                Err(_) => return Err("archive listing output did not close".to_string()),
            };
            if !within_limit {
                return Err(format!(
                    "archive listing exceeds {} bytes",
                    MAX_COLLECTION_LISTING_BYTES
                ));
            }
            break output;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("poll archive listing helper: {error}"))?
        {
            break status;
        }
        if start.elapsed() >= timeout {
            terminate_archive_helper(&mut child);
            return Err(format!(
                "archive listing helper exceeded {} ms",
                timeout.as_millis()
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    if !status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output).into_owned()))
}

#[cfg(any(test, feature = "builder"))]
fn terminate_archive_helper(child: &mut std::process::Child) {
    let _ = child.kill();
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(any(test, feature = "builder"))]
fn result_is_within_limit(length: usize) -> bool {
    length <= MAX_COLLECTION_LISTING_BYTES
}

pub(crate) fn normalize_match_path(path: &str) -> String {
    path.split("::")
        .next()
        .unwrap_or(path)
        .trim()
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

#[cfg(feature = "builder")]
pub(crate) fn parenthesized_setname(path: &str) -> Option<String> {
    let stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    let open = stem.rfind('(')?;
    let close = stem[open + 1..].find(')')? + open + 1;
    let value = stem[open + 1..close].trim();
    if value.is_empty() {
        None
    } else {
        Some(crate::library_db::normalize_id(value))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MraMetadata {
    pub name: Option<String>,
    pub rbf: Option<String>,
    pub platform: Option<String>,
    pub manufacturer: Option<String>,
    pub year: Option<String>,
    pub setname: Option<String>,
    pub parent: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize)]
pub enum RomNamespace {
    Mame,
    Hbmame,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PrimaryRomRequirement {
    None,
    Archive {
        namespace: RomNamespace,
        setname: String,
    },
    Ambiguous,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MraInspection {
    pub header: MraMetadata,
    pub primary_rom: PrimaryRomRequirement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_metadata: Option<crate::arcade_updater_index::ArcadeUpdaterCatalogMetadata>,
}

#[derive(Default)]
pub(crate) struct MglMetadata {
    pub(crate) rbf: Option<String>,
    pub(crate) setname: Option<String>,
    pub(crate) file_path: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct MglFileAction {
    pub(crate) path: String,
    pub(crate) index: Option<u8>,
    pub(crate) kind: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct MglInspection {
    pub(crate) rbf: Option<String>,
    pub(crate) setname: Option<String>,
    pub(crate) files: Vec<MglFileAction>,
    pub(crate) reset_count: usize,
}

#[cfg(test)]
pub(crate) fn read_mra_metadata(path: &Path) -> Option<MraMetadata> {
    let file = File::open(path).ok()?;
    parse_mra_metadata_xml_reader(BufReader::new(file.take(MRA_PREFIX_BYTES as u64)))
}

pub(crate) fn parse_mra_metadata_bytes(data: &[u8]) -> Option<MraMetadata> {
    parse_mra_metadata_xml_reader(BufReader::new(data))
}

pub(crate) fn inspect_mra_bytes(data: &[u8]) -> Result<MraInspection, String> {
    let header = parse_mra_metadata_bytes(data).ok_or("missing MRA metadata")?;
    let mut archive_groups = Vec::<Vec<(RomNamespace, String)>>::new();
    let text = String::from_utf8_lossy(data);
    let lower = text.to_ascii_lowercase();
    if !lower.contains("<misterromdescription") {
        return Err("missing misterromdescription root".to_string());
    }
    for tag in tolerant_mra_rom_tags(&text, &lower) {
        let archives = tolerant_xml_attribute(tag, "zip")
            .into_iter()
            .flat_map(|value| {
                value
                    .split('|')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter_map(|archive| normalize_rom_archive(&archive))
            .collect::<Vec<_>>();
        if !archives.is_empty() {
            archive_groups.push(archives);
        }
    }
    let mut archives = archive_groups.iter().flatten().cloned().collect::<Vec<_>>();
    archives.sort();
    archives.dedup();
    let primary_rom = if archives.is_empty() {
        PrimaryRomRequirement::None
    } else if let Some(setname) = header
        .setname
        .as_deref()
        .map(normalize_rom_setname)
        .filter(|setname| !setname.is_empty())
    {
        let matches = archives
            .iter()
            .filter(|(_, candidate)| candidate == &setname)
            .cloned()
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [(namespace, setname)] => PrimaryRomRequirement::Archive {
                namespace: namespace.clone(),
                setname: setname.clone(),
            },
            [] if archive_groups.len() == 1 => {
                let (namespace, setname) = &archive_groups[0][0];
                PrimaryRomRequirement::Archive {
                    namespace: namespace.clone(),
                    setname: setname.clone(),
                }
            }
            _ => PrimaryRomRequirement::Ambiguous,
        }
    } else {
        match archives.as_slice() {
            [(namespace, setname)] => PrimaryRomRequirement::Archive {
                namespace: namespace.clone(),
                setname: setname.clone(),
            },
            _ => PrimaryRomRequirement::Ambiguous,
        }
    };
    Ok(MraInspection {
        header,
        primary_rom,
        catalog_metadata: None,
    })
}

fn tolerant_mra_rom_tags<'a>(text: &'a str, lower: &str) -> Vec<&'a str> {
    let mut tags = Vec::new();
    let mut offset = 0usize;
    while let Some(relative) = lower[offset..].find('<') {
        let start = offset + relative;
        let Some(end_relative) = lower[start..].find('>') else {
            break;
        };
        let end = start + end_relative + 1;
        let tag_lower = &lower[start + 1..end - 1].trim_start();
        let name_end = tag_lower
            .find(|character: char| character.is_ascii_whitespace() || character == '/')
            .unwrap_or(tag_lower.len());
        if matches!(&tag_lower[..name_end], "rom" | "part") {
            tags.push(&text[start..end]);
        }
        offset = end;
    }
    tags
}

fn tolerant_xml_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut offset = 0usize;
    while let Some(relative) = lower[offset..].find(name) {
        let start = offset + relative;
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let mut cursor = start + name.len();
        let after_ok = cursor >= bytes.len() || !bytes[cursor].is_ascii_alphanumeric();
        if before_ok && after_ok {
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'=') {
                cursor += 1;
                while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                    cursor += 1;
                }
                let quote = *bytes.get(cursor)?;
                if matches!(quote, b'\'' | b'"') {
                    let value_start = cursor + 1;
                    let value_end = bytes[value_start..]
                        .iter()
                        .position(|byte| *byte == quote)?
                        + value_start;
                    return tag.get(value_start..value_end);
                }
            }
        }
        offset = start + name.len();
    }
    None
}

fn normalize_rom_archive(value: &str) -> Option<(RomNamespace, String)> {
    let normalized = value.trim().trim_start_matches('/').replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    let namespace = if lower.starts_with("hbmame/") {
        RomNamespace::Hbmame
    } else {
        RomNamespace::Mame
    };
    let filename = lower.rsplit('/').next()?;
    let setname = filename.strip_suffix(".zip")?;
    if setname.is_empty() {
        None
    } else {
        Some((namespace, normalize_rom_setname(setname)))
    }
}

fn normalize_rom_setname(value: &str) -> String {
    value.trim().trim_end_matches(".zip").to_ascii_lowercase()
}

pub(crate) fn read_mgl_metadata(path: &Path) -> Option<MglMetadata> {
    let data = read_mgl_prefix(path).ok()?;
    parse_mgl_metadata_xml(&String::from_utf8_lossy(&data))
}

pub(crate) fn inspect_mgl(path: &Path) -> Result<MglInspection, String> {
    let data = read_mgl_prefix(path)?;
    inspect_mgl_xml(&String::from_utf8_lossy(&data))
        .map_err(|e| format!("inspect MGL {}: {e}", path.display()))
}

fn read_mgl_prefix(path: &Path) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| format!("open MGL {}: {e}", path.display()))?;
    let mut data = Vec::with_capacity(MGL_PREFIX_BYTES);
    file.take(MGL_PREFIX_BYTES as u64)
        .read_to_end(&mut data)
        .map_err(|e| format!("read MGL {}: {e}", path.display()))?;
    Ok(data)
}

fn inspect_mgl_xml(text: &str) -> Result<MglInspection, String> {
    let mut reader = XmlReader::from_str(text);
    let mut inspection = MglInspection::default();
    let mut text_tag: Option<&'static str> = None;
    let mut text_value = String::new();
    let mut pending_file: Option<MglFileAction> = None;
    let mut saw_root = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name();
                if xml_name_eq(name.as_ref(), b"mistergamedescription")
                    || xml_name_eq(name.as_ref(), b"mistergamelist")
                {
                    saw_root = true;
                } else if xml_name_eq(name.as_ref(), b"rbf") {
                    text_tag = Some("rbf");
                    text_value.clear();
                } else if xml_name_eq(name.as_ref(), b"setname") {
                    text_tag = Some("setname");
                    text_value.clear();
                } else if xml_name_eq(name.as_ref(), b"file") {
                    pending_file = Some(mgl_file_action_from_element(&e));
                    text_tag = Some("file");
                    text_value.clear();
                } else if xml_name_eq(name.as_ref(), b"reset") {
                    inspection.reset_count = inspection.reset_count.saturating_add(1);
                }
            }
            Ok(Event::Empty(e)) => {
                if xml_name_eq(e.name().as_ref(), b"file") {
                    let action = mgl_file_action_from_element(&e);
                    if !action.path.is_empty() {
                        inspection.files.push(action);
                    }
                } else if xml_name_eq(e.name().as_ref(), b"reset") {
                    inspection.reset_count = inspection.reset_count.saturating_add(1);
                }
            }
            Ok(Event::Text(e)) => {
                if text_tag.is_some() {
                    text_value.push_str(&e.xml10_content());
                }
            }
            Ok(Event::CData(e)) => {
                if text_tag.is_some() {
                    text_value.push_str(&e.xml10_content());
                }
            }
            Ok(Event::End(e)) => {
                if xml_name_eq(e.name().as_ref(), b"rbf") && text_tag == Some("rbf") {
                    set_optional_trimmed(&mut inspection.rbf, &text_value);
                    text_tag = None;
                } else if xml_name_eq(e.name().as_ref(), b"setname") && text_tag == Some("setname")
                {
                    set_optional_trimmed(&mut inspection.setname, &text_value);
                    text_tag = None;
                } else if xml_name_eq(e.name().as_ref(), b"file") && text_tag == Some("file") {
                    if let Some(mut action) = pending_file.take() {
                        if action.path.is_empty() {
                            action.path = text_value.trim().to_string();
                        }
                        if !action.path.is_empty() {
                            inspection.files.push(action);
                        }
                    }
                    text_tag = None;
                }
                text_value.clear();
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("invalid XML: {error}")),
            _ => {}
        }
    }
    if !saw_root {
        return Err("missing mistergamedescription root".to_string());
    }
    Ok(inspection)
}

fn mgl_file_action_from_element(element: &BytesStart<'_>) -> MglFileAction {
    MglFileAction {
        path: xml_attr_value(element, b"path").unwrap_or_default(),
        index: xml_attr_value(element, b"index").and_then(|value| value.parse().ok()),
        kind: xml_attr_value(element, b"type"),
    }
}

pub(crate) fn resolve_mgl_payload_path(mgl_path: &Path, payload: &str) -> PathBuf {
    if payload.starts_with('/') {
        PathBuf::from(payload)
    } else if payload.starts_with("games/") {
        PathBuf::from("/media/fat").join(payload)
    } else {
        mgl_path.parent().unwrap_or(Path::new("/")).join(payload)
    }
}

fn parse_mra_metadata_xml_reader(reader: impl BufRead) -> Option<MraMetadata> {
    let mut reader = XmlReader::from_reader(reader);
    let mut buf = Vec::with_capacity(4096);
    let mut metadata = MraMetadata::default();
    let mut field: Option<&'static str> = None;
    let mut field_text = String::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) | Err(_) => break,
            Ok(event) => {
                if !apply_mra_metadata_event(event, &mut metadata, &mut field, &mut field_text) {
                    break;
                }
            }
        }
    }
    Some(metadata)
}

fn apply_mra_metadata_event(
    event: Event<'_>,
    metadata: &mut MraMetadata,
    field: &mut Option<&'static str>,
    field_text: &mut String,
) -> bool {
    match event {
        Event::Start(e) => {
            // The MRA metadata contract places the descriptive header before
            // the first ROM payload. ROM elements can contain a large number
            // of parts and patches, none of which affect catalog projection,
            // so do not pull that payload from exFAT during discovery.
            if xml_name_eq(e.name().as_ref(), b"rom") {
                return false;
            }
            *field = mra_metadata_field(e.name().as_ref());
            field_text.clear();
        }
        Event::Empty(e) if xml_name_eq(e.name().as_ref(), b"rom") => return false,
        Event::Text(e) => {
            if field.is_some() {
                let value = e.xml10_content();
                field_text.push_str(&value);
            }
        }
        Event::CData(e) => {
            if field.is_some() {
                let value = e.xml10_content();
                field_text.push_str(&value);
            }
        }
        Event::GeneralRef(e) => {
            if field.is_some()
                && let Some(value) = xml_general_ref_text(e.as_ref())
            {
                field_text.push_str(value);
            }
        }
        Event::End(e) => {
            if let Some(ended_field) = mra_metadata_field(e.name().as_ref()) {
                if *field == Some(ended_field) {
                    set_mra_metadata_field(metadata, ended_field, field_text);
                }
                *field = None;
                field_text.clear();
            } else if xml_name_eq(e.name().as_ref(), b"misterromdescription") {
                return false;
            }
        }
        _ => {}
    }
    true
}

fn parse_mgl_metadata_xml(text: &str) -> Option<MglMetadata> {
    let mut reader = XmlReader::from_str(text);
    let mut metadata = MglMetadata::default();
    let mut in_rbf = false;
    let mut in_setname = false;
    let mut in_file = false;
    let mut rbf_text = String::new();
    let mut setname_text = String::new();
    let mut file_text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = e.name();
                if xml_name_eq(tag.as_ref(), b"rbf") {
                    in_rbf = true;
                    rbf_text.clear();
                } else if xml_name_eq(tag.as_ref(), b"setname") {
                    in_setname = true;
                    setname_text.clear();
                } else if xml_name_eq(tag.as_ref(), b"file") && metadata.file_path.is_none() {
                    in_file = true;
                    file_text.clear();
                    metadata.file_path = xml_attr_value(&e, b"path");
                }
            }
            Ok(Event::Empty(e)) => {
                if xml_name_eq(e.name().as_ref(), b"file") && metadata.file_path.is_none() {
                    metadata.file_path = xml_attr_value(&e, b"path");
                }
            }
            Ok(Event::Text(e)) => {
                let value = e.xml10_content();
                if in_rbf {
                    rbf_text.push_str(&value);
                } else if in_setname {
                    setname_text.push_str(&value);
                } else if in_file && metadata.file_path.is_none() {
                    file_text.push_str(&value);
                }
            }
            Ok(Event::CData(e)) => {
                let value = e.xml10_content();
                if in_rbf {
                    rbf_text.push_str(&value);
                } else if in_setname {
                    setname_text.push_str(&value);
                } else if in_file && metadata.file_path.is_none() {
                    file_text.push_str(&value);
                }
            }
            Ok(Event::GeneralRef(e)) => {
                if let Some(value) = xml_general_ref_text(e.as_ref()) {
                    if in_rbf {
                        rbf_text.push_str(value);
                    } else if in_setname {
                        setname_text.push_str(value);
                    } else if in_file && metadata.file_path.is_none() {
                        file_text.push_str(value);
                    }
                }
            }
            Ok(Event::End(e)) => {
                if xml_name_eq(e.name().as_ref(), b"rbf") {
                    set_optional_trimmed(&mut metadata.rbf, &rbf_text);
                    in_rbf = false;
                    rbf_text.clear();
                } else if xml_name_eq(e.name().as_ref(), b"setname") {
                    set_optional_trimmed(&mut metadata.setname, &setname_text);
                    in_setname = false;
                    setname_text.clear();
                } else if xml_name_eq(e.name().as_ref(), b"file") {
                    if metadata.file_path.is_none() {
                        set_optional_trimmed(&mut metadata.file_path, &file_text);
                    }
                    in_file = false;
                    file_text.clear();
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    Some(metadata)
}

fn mra_metadata_field(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "name" => Some("name"),
        "rbf" => Some("rbf"),
        "platform" => Some("platform"),
        "manufacturer" => Some("manufacturer"),
        "year" => Some("year"),
        "setname" => Some("setname"),
        "parent" => Some("parent"),
        _ => None,
    }
}

fn set_mra_metadata_field(metadata: &mut MraMetadata, field: &str, value: &str) {
    match field {
        "name" => set_optional_trimmed(&mut metadata.name, value),
        "rbf" => set_optional_trimmed(&mut metadata.rbf, value),
        "platform" => set_optional_trimmed(&mut metadata.platform, value),
        "manufacturer" => set_optional_trimmed(&mut metadata.manufacturer, value),
        "year" => set_optional_trimmed(&mut metadata.year, value),
        "setname" => set_optional_trimmed(&mut metadata.setname, value),
        "parent" => set_optional_trimmed(&mut metadata.parent, value),
        _ => {}
    }
}

fn set_optional_trimmed(slot: &mut Option<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        *slot = Some(value.to_string());
    }
}

fn xml_name_eq(name: &str, expected: &[u8]) -> bool {
    name.as_bytes().eq_ignore_ascii_case(expected)
}

fn xml_attr_value(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|attr| attr.key.as_ref().as_bytes().eq_ignore_ascii_case(key))
        .and_then(|attr| {
            attr.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|value| value.into_owned())
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn xml_general_ref_text(name: &str) -> Option<&'static str> {
    match name {
        "amp" => Some("&"),
        "quot" => Some("\""),
        "apos" => Some("'"),
        "lt" => Some("<"),
        "gt" => Some(">"),
        _ => None,
    }
}

pub(crate) fn is_amigavision_listing_path(path: &Path) -> bool {
    let path = normalize_match_path(&path.display().to_string());
    path.ends_with("/games/amiga/listings/games.txt")
        || path.ends_with("/games/amiga/listings/demos.txt")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use std::time::Duration;

    #[test]
    fn mra_metadata_parser_tolerates_attributes_and_entities() {
        let root = unique_temp_dir("mra-xml-metadata");
        std::fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("fixture.mra");
        std::fs::write(
            &path,
            r#"
            <misterromdescription>
                <name lang="en">Battle &amp; Chase</name>
                <rbf version="1">JTCPS2</rbf>
                <platform>Capcom Play System II</platform>
                <manufacturer>Capcom &quot;Co&quot;</manufacturer>
                <year>1997</year>
                <setname>batcir</setname>
                <parent>batcirj</parent>
            </misterromdescription>
            "#,
        )
        .expect("write mra fixture");

        let metadata = read_mra_metadata(&path).expect("read mra metadata");

        assert_eq!(metadata.name.as_deref(), Some("Battle & Chase"));
        assert_eq!(metadata.rbf.as_deref(), Some("JTCPS2"));
        assert_eq!(metadata.platform.as_deref(), Some("Capcom Play System II"));
        assert_eq!(metadata.manufacturer.as_deref(), Some("Capcom \"Co\""));
        assert_eq!(metadata.year.as_deref(), Some("1997"));
        assert_eq!(metadata.setname.as_deref(), Some("batcir"));
        assert_eq!(metadata.parent.as_deref(), Some("batcirj"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mra_metadata_reader_ignores_trailing_payload_after_root() {
        let root = unique_temp_dir("mra-trailing-payload");
        std::fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("fixture.mra");
        let mut data = br#"
            <misterromdescription>
                <name>Fast Game</name>
                <rbf>Arcade</rbf>
            </misterromdescription>
            "#
        .to_vec();
        data.extend_from_slice(&[0xff, 0x00, 0xfe, b'<', b'b', b'a', b'd']);
        std::fs::write(&path, data).expect("write mra fixture");

        let metadata = read_mra_metadata(&path).expect("read mra metadata");

        assert_eq!(metadata.name.as_deref(), Some("Fast Game"));
        assert_eq!(metadata.rbf.as_deref(), Some("Arcade"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mra_inspection_prefers_setname_over_parent_and_bios_archives() {
        let inspection = inspect_mra_bytes(
            br#"<misterromdescription>
                <name>Dragon World 3</name><setname>drgw3105</setname>
                <rom zip="pgm.zip|drgw3.zip|drgw3105.zip"><part name="game.bin"/></rom>
            </misterromdescription>"#,
        )
        .unwrap();

        assert_eq!(
            inspection.primary_rom,
            PrimaryRomRequirement::Archive {
                namespace: RomNamespace::Mame,
                setname: "drgw3105".to_string(),
            }
        );
    }

    #[test]
    fn mra_inspection_uses_first_archive_when_setname_is_only_display_identity() {
        let inspection = inspect_mra_bytes(
            br#"<misterromdescription>
                <name>Space Demon</name><setname>SpaceDemon</setname>
                <rom zip="spacedem.zip|spacefb.zip|bios.zip"><part name="game.bin"/></rom>
            </misterromdescription>"#,
        )
        .unwrap();

        assert_eq!(
            inspection.primary_rom,
            PrimaryRomRequirement::Archive {
                namespace: RomNamespace::Mame,
                setname: "spacedem".to_string(),
            }
        );
    }

    #[test]
    fn mra_inspection_keeps_multiple_unmatched_archive_declarations_ambiguous() {
        let inspection = inspect_mra_bytes(
            br#"<misterromdescription><setname>display-id</setname>
                <rom zip="one.zip|parent.zip"><part/></rom>
                <rom zip="two.zip"><part/></rom>
            </misterromdescription>"#,
        )
        .unwrap();

        assert_eq!(inspection.primary_rom, PrimaryRomRequirement::Ambiguous);
    }

    #[test]
    fn mra_inspection_respects_hbmame_and_rejects_ambiguous_requirements() {
        let hbmame = inspect_mra_bytes(
            br#"<misterromdescription><setname>asteroid01</setname>
                <rom zip="/hbmame/asteroid01.zip"><part/></rom>
            </misterromdescription>"#,
        )
        .unwrap();
        assert_eq!(
            hbmame.primary_rom,
            PrimaryRomRequirement::Archive {
                namespace: RomNamespace::Hbmame,
                setname: "asteroid01".to_string(),
            }
        );

        let ambiguous = inspect_mra_bytes(
            br#"<misterromdescription><rom zip="one.zip|two.zip"><part/></rom></misterromdescription>"#,
        )
        .unwrap();
        assert_eq!(ambiguous.primary_rom, PrimaryRomRequirement::Ambiguous);
    }

    #[test]
    fn mra_inspection_preserves_embedded_rom_launchers() {
        let inspection = inspect_mra_bytes(
            br#"<misterromdescription><name>Embedded</name><rom index="0"><part>00</part></rom></misterromdescription>"#,
        )
        .unwrap();
        assert_eq!(inspection.primary_rom, PrimaryRomRequirement::None);
    }

    #[test]
    fn mra_metadata_parser_stops_before_rom_payload_bytes() {
        use std::cell::Cell;
        use std::io::{BufReader, Cursor, Read};
        use std::rc::Rc;

        struct CountingReader {
            cursor: Cursor<Vec<u8>>,
            bytes_read: Rc<Cell<usize>>,
        }

        impl Read for CountingReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let read = self.cursor.read(buf)?;
                self.bytes_read.set(self.bytes_read.get() + read);
                Ok(read)
            }
        }

        let mut document = br#"<misterromdescription>
            <name>Fast Game</name>
            <rbf>Arcade</rbf>
            <platform>Arcade</platform>
            <rom index="0">"#
            .to_vec();
        document.extend(std::iter::repeat_n(b'x', 128 * 1024));
        document.extend_from_slice(b"</rom></misterromdescription>");
        let document_len = document.len();
        let bytes_read = Rc::new(Cell::new(0));
        let reader = CountingReader {
            cursor: Cursor::new(document),
            bytes_read: Rc::clone(&bytes_read),
        };

        let metadata = parse_mra_metadata_xml_reader(BufReader::with_capacity(512, reader))
            .expect("MRA metadata");

        assert_eq!(metadata.name.as_deref(), Some("Fast Game"));
        assert_eq!(metadata.rbf.as_deref(), Some("Arcade"));
        assert_eq!(metadata.platform.as_deref(), Some("Arcade"));
        assert!(bytes_read.get() < document_len / 100);
    }

    #[test]
    fn mgl_metadata_parser_uses_file_path_not_unrelated_path_attribute() {
        let root = unique_temp_dir("mgl-xml-file-path");
        std::fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("Fixture.mgl");
        std::fs::write(
            &path,
            r#"
            <mistergamelist>
                <metadata path="not/a/game.rom"/>
                <rbf>NES</rbf>
                <file delay="1" type="s" path='games/NES/Super Mario Bros.nes'/>
            </mistergamelist>
            "#,
        )
        .expect("write mgl fixture");

        let metadata = read_mgl_metadata(&path).expect("read mgl metadata");

        assert_eq!(metadata.rbf.as_deref(), Some("NES"));
        assert_eq!(
            metadata.file_path.as_deref(),
            Some("games/NES/Super Mario Bros.nes")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mgl_metadata_parser_reads_system_setname() {
        let root = unique_temp_dir("mgl-system-setname");
        std::fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("Atari 2600.mgl");
        std::fs::write(
            &path,
            r#"<mistergamedescription><rbf>_Console/Atari7800</rbf><setname>Atari2600</setname></mistergamedescription>"#,
        )
        .expect("write mgl fixture");

        let metadata = read_mgl_metadata(&path).expect("read mgl metadata");

        assert_eq!(metadata.rbf.as_deref(), Some("_Console/Atari7800"));
        assert_eq!(metadata.setname.as_deref(), Some("Atari2600"));
        assert_eq!(metadata.file_path, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mgl_metadata_parser_reads_file_text_payload() {
        let root = unique_temp_dir("mgl-file-text");
        std::fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("Fixture.mgl");
        std::fs::write(
            &path,
            r#"<mistergamelist><rbf>NES</rbf><file delay="1" type="s">../games/NES/Mario.nes</file></mistergamelist>"#,
        )
        .expect("write mgl fixture");

        let metadata = read_mgl_metadata(&path).expect("read mgl metadata");

        assert_eq!(metadata.rbf.as_deref(), Some("NES"));
        assert_eq!(
            metadata.file_path.as_deref(),
            Some("../games/NES/Mario.nes")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mgl_document_preserves_lenient_metadata_when_strict_inspection_fails() {
        let root = unique_temp_dir("mgl-document-lenient");
        std::fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("Fixture.mgl");
        std::fs::write(&path, r#"<rbf>NES</rbf><file path="games/NES/Mario.nes"/>"#)
            .expect("write rootless mgl fixture");

        let metadata = read_mgl_metadata(&path).expect("read MGL metadata");

        assert_eq!(metadata.rbf.as_deref(), Some("NES"));
        assert_eq!(metadata.file_path.as_deref(), Some("games/NES/Mario.nes"));
        assert!(
            inspect_mgl(&path)
                .expect_err("rootless MGL must fail strict inspection")
                .contains("missing mistergamedescription root")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mgl_metadata_reader_uses_bounded_prefix() {
        let root = unique_temp_dir("mgl-bounded-prefix");
        std::fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("Fixture.mgl");
        let mut data = br#"
            <mistergamelist>
                <rbf>NES</rbf>
                <file delay="1" type="s" path="games/NES/Super Mario Bros.nes"/>
            </mistergamelist>
            "#
        .to_vec();
        data.resize(MGL_PREFIX_BYTES + 128, b' ');
        data.extend_from_slice(&[0xff, 0xfe, 0xfd]);
        std::fs::write(&path, data).expect("write mgl fixture");

        let metadata = read_mgl_metadata(&path).expect("read bounded mgl metadata");

        assert_eq!(metadata.rbf.as_deref(), Some("NES"));
        assert_eq!(
            metadata.file_path.as_deref(),
            Some("games/NES/Super Mario Bros.nes")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn collection_listing_helper_times_out() {
        use std::os::unix::fs::PermissionsExt;

        let root = unique_temp_dir("collection-listing-timeout");
        let helper = root.join("slow-7za.sh");
        std::fs::write(&helper, "#!/bin/sh\nsleep 2\n").expect("write helper");
        let mut permissions = std::fs::metadata(&helper)
            .expect("stat helper")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&helper, permissions).expect("chmod helper");
        let archive = root.join("AmigaVision.7z");
        std::fs::write(&archive, "fixture").expect("write archive fixture");
        let file = FoundFile {
            path: archive,
            ext: "7z".to_string(),
        };
        let listing = CollectionListing {
            entry_path: "listings/games.txt".to_string(),
            genre: "AmigaVision".to_string(),
        };
        let start = Instant::now();

        let text = collection_listing_text_with_tool_result(
            &file,
            &listing,
            &helper,
            Duration::from_millis(75),
        );

        assert!(text.is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn collection_listing_helper_rejects_unbounded_output() {
        use std::os::unix::fs::PermissionsExt;

        let root = unique_temp_dir("collection-listing-output-limit");
        let helper = root.join("large-7za.sh");
        std::fs::write(&helper, "#!/bin/sh\nhead -c 9000000 /dev/zero\n").expect("write helper");
        let mut permissions = std::fs::metadata(&helper)
            .expect("stat helper")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&helper, permissions).expect("chmod helper");
        let archive = root.join("AmigaVision.7z");
        std::fs::write(&archive, "fixture").expect("write archive fixture");
        let file = FoundFile {
            path: archive,
            ext: "7z".to_string(),
        };
        let listing = CollectionListing {
            entry_path: "listings/games.txt".to_string(),
            genre: "AmigaVision".to_string(),
        };

        assert!(
            collection_listing_text_with_tool_result(
                &file,
                &listing,
                &helper,
                Duration::from_secs(1),
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
