//! Media metadata parsing helpers.

use crate::catalog_scan::FoundFile;
use crate::launch_profiles::{CollectionListing, CollectionRule, LaunchProfile};
use crate::library_db::{
    DiscoveryConfidence, DiscoverySourceKind, GameDiscovery, AMIGAVISION_GAME_LAUNCH_PREFIX,
    AMIGAVISION_INSTALLED_LISTINGS, AMIGAVISION_LAUNCHER_REF, MRA_PREFIX_BYTES,
};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader as XmlReader;
use quick_xml::XmlVersion;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_COLLECTION_LISTING_TIMEOUT_SECS: u64 = 1;

pub(crate) fn collection_discoveries_from_container(
    file: &FoundFile,
    profile: &LaunchProfile,
    rule: &CollectionRule,
) -> Vec<GameDiscovery> {
    let mut out = Vec::new();
    if is_amigavision_archive_path(&file.path.display().to_string()) {
        out.push(amigavision_launcher_discovery(file, profile));
    }
    for listing in rule.listings {
        let text = match collection_listing_text(file, listing) {
            Some(text) => text,
            None => continue,
        };
        out.extend(collection_discoveries_from_listing_text(
            file, profile, listing, &text,
        ));
    }
    out
}

pub(crate) fn installed_amigavision_discoveries_from_hdf(
    file: &FoundFile,
    profile: &LaunchProfile,
) -> Option<Vec<GameDiscovery>> {
    if !is_amigavision_installed_hdf_path(&file.path) {
        return None;
    }
    let mut out = vec![amigavision_launcher_discovery(file, profile)];
    for listing in AMIGAVISION_INSTALLED_LISTINGS {
        let Some(listing_path) = installed_amigavision_listing_path(&file.path, listing) else {
            continue;
        };
        let Some(text) = read_lossy_text(&listing_path) else {
            continue;
        };
        out.extend(collection_discoveries_from_listing_text(
            file, profile, listing, &text,
        ));
    }
    Some(out)
}

fn installed_amigavision_listing_path(
    hdf_path: &Path,
    listing: &CollectionListing,
) -> Option<PathBuf> {
    let base = hdf_path.parent()?;
    let relative = listing
        .entry_path
        .strip_prefix("games/Amiga/")
        .unwrap_or(listing.entry_path);
    Some(base.join(relative))
}

fn read_lossy_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn amigavision_launcher_discovery(file: &FoundFile, profile: &LaunchProfile) -> GameDiscovery {
    GameDiscovery {
        source_path: file.path.display().to_string(),
        launch_ref: AMIGAVISION_LAUNCHER_REF.to_string(),
        source_kind: DiscoverySourceKind::CatalogEntry,
        title: "AmigaVision".to_string(),
        category: profile.category.to_string(),
        platform_id: profile.system_id.to_string(),
        core_id: profile.core_name.to_string(),
        hardware_id: profile.system_id.to_string(),
        manufacturer: Some("Commodore".to_string()),
        genre: Some("Launcher".to_string()),
        year: None,
        setname: None,
        parent: None,
        confidence: DiscoveryConfidence::CatalogMetadata,
    }
}

fn collection_listing_text(file: &FoundFile, listing: &CollectionListing) -> Option<String> {
    let tool = std::env::var("MISTER_7ZA").unwrap_or_else(|_| "/media/fat/linux/7za".to_string());
    collection_listing_text_with_tool(
        file,
        listing,
        Path::new(&tool),
        collection_listing_timeout(),
    )
}

fn collection_listing_timeout() -> Duration {
    let secs = std::env::var("MISTER_7ZA_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_COLLECTION_LISTING_TIMEOUT_SECS)
        .clamp(1, 120);
    Duration::from_secs(secs)
}

pub(crate) fn collection_listing_text_with_tool(
    file: &FoundFile,
    listing: &CollectionListing,
    tool: &Path,
    timeout: Duration,
) -> Option<String> {
    let mut child = Command::new(tool)
        .args(["e", "-so"])
        .arg(&file.path)
        .arg(listing.entry_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        if child.try_wait().ok()?.is_some() {
            let output = child.wait_with_output().ok()?;
            if !output.status.success() {
                return None;
            }
            return Some(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub(crate) fn collection_discoveries_from_listing_text(
    file: &FoundFile,
    profile: &LaunchProfile,
    listing: &CollectionListing,
    text: &str,
) -> Vec<GameDiscovery> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|title| GameDiscovery {
            source_path: format!("{}::{}::{title}", file.path.display(), listing.entry_path),
            launch_ref: amigavision_game_launch_ref(title),
            source_kind: DiscoverySourceKind::CatalogEntry,
            title: title.to_string(),
            category: profile.category.to_string(),
            platform_id: profile.system_id.to_string(),
            core_id: profile.core_name.to_string(),
            hardware_id: profile.system_id.to_string(),
            manufacturer: Some("Commodore".to_string()),
            genre: Some(listing.genre.to_string()),
            year: None,
            setname: None,
            parent: None,
            confidence: DiscoveryConfidence::CatalogMetadata,
        })
        .collect()
}

pub(crate) fn normalize_match_path(path: &str) -> String {
    path.split("::")
        .next()
        .unwrap_or(path)
        .trim()
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

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

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[derive(Default)]
pub(crate) struct MraMetadata {
    pub(crate) name: Option<String>,
    pub(crate) rbf: Option<String>,
    pub(crate) platform: Option<String>,
    pub(crate) manufacturer: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) catver: Option<String>,
    pub(crate) year: Option<String>,
    pub(crate) setname: Option<String>,
    pub(crate) parent: Option<String>,
}

#[derive(Default)]
pub(crate) struct MglMetadata {
    pub(crate) rbf: Option<String>,
    pub(crate) file_path: Option<String>,
}

pub(crate) fn read_mra_metadata(path: &Path) -> Option<MraMetadata> {
    let mut file = File::open(path).ok()?;
    let mut data = vec![0u8; MRA_PREFIX_BYTES];
    let n = file.read(&mut data).ok()?;
    data.truncate(n);
    parse_mra_metadata_xml(&String::from_utf8_lossy(&data))
}

pub(crate) fn read_mgl_metadata(path: &Path) -> Option<MglMetadata> {
    let mut file = File::open(path).ok()?;
    let mut data = String::new();
    file.read_to_string(&mut data).ok()?;
    parse_mgl_metadata_xml(&data)
}

pub(crate) fn neogeo_mgl_setname(mgl_path: &Path, payload_path: Option<&str>) -> Option<String> {
    payload_path
        .and_then(parenthesized_setname)
        .or_else(|| parenthesized_setname(&mgl_path.display().to_string()))
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

fn parse_mra_metadata_xml(text: &str) -> Option<MraMetadata> {
    let mut reader = XmlReader::from_str(text);
    let mut metadata = MraMetadata::default();
    let mut field: Option<&'static str> = None;
    let mut field_text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                field = mra_metadata_field(e.name().as_ref());
                field_text.clear();
            }
            Ok(Event::Text(e)) => {
                if field.is_some() {
                    if let Ok(value) = e.xml10_content() {
                        field_text.push_str(&value);
                    }
                }
            }
            Ok(Event::CData(e)) => {
                if field.is_some() {
                    if let Ok(value) = e.xml10_content() {
                        field_text.push_str(&value);
                    }
                }
            }
            Ok(Event::GeneralRef(e)) => {
                if field.is_some() {
                    if let Some(value) = xml_general_ref_text(e.as_ref()) {
                        field_text.push_str(value);
                    }
                }
            }
            Ok(Event::End(e)) => {
                if let Some(ended_field) = mra_metadata_field(e.name().as_ref()) {
                    if field == Some(ended_field) {
                        set_mra_metadata_field(&mut metadata, ended_field, &field_text);
                    }
                    field = None;
                    field_text.clear();
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    Some(metadata)
}

fn parse_mgl_metadata_xml(text: &str) -> Option<MglMetadata> {
    let mut reader = XmlReader::from_str(text);
    let mut metadata = MglMetadata::default();
    let mut in_rbf = false;
    let mut rbf_text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = e.name();
                if tag.as_ref().eq_ignore_ascii_case(b"rbf") {
                    in_rbf = true;
                    rbf_text.clear();
                } else if tag.as_ref().eq_ignore_ascii_case(b"file") && metadata.file_path.is_none()
                {
                    metadata.file_path = xml_attr_value(&e, b"path");
                }
            }
            Ok(Event::Empty(e)) => {
                if e.name().as_ref().eq_ignore_ascii_case(b"file") && metadata.file_path.is_none() {
                    metadata.file_path = xml_attr_value(&e, b"path");
                }
            }
            Ok(Event::Text(e)) => {
                if in_rbf {
                    if let Ok(value) = e.xml10_content() {
                        rbf_text.push_str(&value);
                    }
                }
            }
            Ok(Event::CData(e)) => {
                if in_rbf {
                    if let Ok(value) = e.xml10_content() {
                        rbf_text.push_str(&value);
                    }
                }
            }
            Ok(Event::GeneralRef(e)) => {
                if in_rbf {
                    if let Some(value) = xml_general_ref_text(e.as_ref()) {
                        rbf_text.push_str(value);
                    }
                }
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref().eq_ignore_ascii_case(b"rbf") {
                    set_optional_trimmed(&mut metadata.rbf, &rbf_text);
                    in_rbf = false;
                    rbf_text.clear();
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    Some(metadata)
}

fn mra_metadata_field(name: &[u8]) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_slice() {
        b"name" => Some("name"),
        b"rbf" => Some("rbf"),
        b"platform" => Some("platform"),
        b"manufacturer" => Some("manufacturer"),
        b"category" => Some("category"),
        b"catver" => Some("catver"),
        b"year" => Some("year"),
        b"setname" => Some("setname"),
        b"parent" => Some("parent"),
        _ => None,
    }
}

fn set_mra_metadata_field(metadata: &mut MraMetadata, field: &str, value: &str) {
    match field {
        "name" => set_optional_trimmed(&mut metadata.name, value),
        "rbf" => set_optional_trimmed(&mut metadata.rbf, value),
        "platform" => set_optional_trimmed(&mut metadata.platform, value),
        "manufacturer" => set_optional_trimmed(&mut metadata.manufacturer, value),
        "category" => set_optional_trimmed(&mut metadata.category, value),
        "catver" => set_optional_trimmed(&mut metadata.catver, value),
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

fn xml_attr_value(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|attr| attr.key.as_ref().eq_ignore_ascii_case(key))
        .and_then(|attr| {
            attr.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|value| value.into_owned())
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn xml_general_ref_text(name: &[u8]) -> Option<&'static str> {
    match name {
        b"amp" => Some("&"),
        b"quot" => Some("\""),
        b"apos" => Some("'"),
        b"lt" => Some("<"),
        b"gt" => Some(">"),
        _ => None,
    }
}

pub(crate) fn amigavision_game_launch_ref(title: &str) -> String {
    format!(
        "{AMIGAVISION_GAME_LAUNCH_PREFIX}{}",
        encode_launch_component(title)
    )
}

fn encode_launch_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub(crate) fn is_amigavision_archive_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/games/amiga/") && lower.contains("amigavision") && lower.ends_with(".7z")
}

pub(crate) fn is_amigavision_installed_hdf_path(path: &Path) -> bool {
    normalize_match_path(&path.display().to_string()).ends_with("/games/amiga/amigavision.hdf")
}

pub(crate) fn is_amigavision_save_media_path(path: &Path) -> bool {
    normalize_match_path(&path.display().to_string())
        .ends_with("/games/amiga/amigavision-saves.hdf")
}

pub(crate) fn is_amigavision_listing_path(path: &Path) -> bool {
    let path = normalize_match_path(&path.display().to_string());
    path.ends_with("/games/amiga/listings/games.txt")
        || path.ends_with("/games/amiga/listings/demos.txt")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegionInference {
    pub(crate) region: Option<&'static str>,
    pub(crate) confidence: &'static str,
}

pub(crate) fn infer_region_metadata(discovery: &GameDiscovery) -> RegionInference {
    if discovery.platform_id != "saturn" {
        return RegionInference {
            region: None,
            confidence: "unknown",
        };
    }

    if let Some(region) = region_from_saturn_boot_header_file(&discovery.source_path) {
        return RegionInference {
            region: Some(region),
            confidence: "disc-header",
        };
    }
    if let Some(region) = region_from_filename(&discovery.source_path) {
        return RegionInference {
            region: Some(region),
            confidence: "filename-high",
        };
    }
    if let Some(region) = region_from_folder(&discovery.source_path) {
        return RegionInference {
            region: Some(region),
            confidence: "folder-medium",
        };
    }
    if let Some(region) = region_from_text(&discovery.title) {
        return RegionInference {
            region: Some(region),
            confidence: "metadata-low",
        };
    }

    RegionInference {
        region: None,
        confidence: "unknown",
    }
}

pub(crate) fn region_from_saturn_boot_header_file(path: &str) -> Option<&'static str> {
    let path = path.split("::").next().unwrap_or(path);
    let mut file = File::open(path).ok()?;
    let mut header = [0u8; 256];
    file.read_exact(&mut header).ok()?;
    parse_saturn_boot_header(&header)?.region
}

pub(crate) fn region_from_filename(path: &str) -> Option<&'static str> {
    let stem = Path::new(path.split("::").next().unwrap_or(path))
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    region_from_text(stem)
}

pub(crate) fn region_from_folder(path: &str) -> Option<&'static str> {
    Path::new(path.split("::").next().unwrap_or(path))
        .parent()?
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .rev()
        .find_map(region_from_text)
}

pub(crate) fn region_from_text(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    let token = lower.trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
    if matches!(token, "usa" | "us" | "u") {
        return Some("usa");
    }
    if matches!(token, "europe" | "eu" | "e") {
        return Some("europe");
    }
    if matches!(token, "japan" | "jp" | "j") {
        return Some("japan");
    }
    if matches!(token, "world" | "w") {
        return Some("world");
    }
    if contains_any(
        &lower,
        &["(usa", "(us)", "(u)", "[usa", "[us]", " usa", " ntsc-u"],
    ) {
        Some("usa")
    } else if contains_any(
        &lower,
        &[
            "(europe", "(eu", "(e)", "[europe", "[eu]", " europe", " pal",
        ],
    ) {
        Some("europe")
    } else if contains_any(
        &lower,
        &[
            "(japan", "(jp", "(j)", "[japan", "[jp]", " japan", " ntsc-j",
        ],
    ) {
        Some("japan")
    } else if contains_any(&lower, &["(world", "(w)", "[world", " world"]) {
        Some("world")
    } else {
        None
    }
}

pub(crate) fn canonical_region_static(region: &str) -> Option<&'static str> {
    match region.trim().to_ascii_lowercase().as_str() {
        "usa" | "us" => Some("usa"),
        "europe" | "eu" => Some("europe"),
        "japan" | "jp" => Some("japan"),
        "korea" | "kr" => Some("korea"),
        "world" => Some("world"),
        "unknown" => Some("unknown"),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SaturnBootHeader {
    pub(crate) product_id: Option<String>,
    pub(crate) region: Option<&'static str>,
}

pub(crate) fn parse_saturn_boot_header(bytes: &[u8]) -> Option<SaturnBootHeader> {
    if bytes.len() < 0x50 || !bytes.starts_with(b"SEGA SEGASATURN") {
        return None;
    }
    let product_id = ascii_trim(&bytes[0x20..0x2a]);
    let area = String::from_utf8_lossy(&bytes[0x40..0x50]).to_ascii_uppercase();
    let region = if area.contains('U') {
        Some("usa")
    } else if area.contains('E') {
        Some("europe")
    } else if area.contains('J') {
        Some("japan")
    } else if area.contains('K') {
        Some("korea")
    } else {
        None
    };
    Some(SaturnBootHeader { product_id, region })
}

fn ascii_trim(bytes: &[u8]) -> Option<String> {
    let value = String::from_utf8_lossy(bytes)
        .trim_matches(|ch: char| ch.is_ascii_whitespace() || ch == '\0')
        .to_string();
    (!value.is_empty()).then_some(value)
}
