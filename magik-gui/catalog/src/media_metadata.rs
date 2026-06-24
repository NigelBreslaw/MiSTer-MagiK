//! Media metadata parsing helpers.

use crate::catalog_scan::FoundFile;
use crate::game_discovery::{DiscoveryConfidence, DiscoverySourceKind, GameDiscovery};
use crate::launch_profiles::{CollectionListing, CollectionRule, LaunchProfile};
use crate::library_db::{
    AMIGAVISION_GAME_LAUNCH_PREFIX, AMIGAVISION_INSTALLED_LISTINGS, AMIGAVISION_LAUNCHER_REF,
    MRA_PREFIX_BYTES,
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
const MGL_PREFIX_BYTES: usize = 32 * 1024;

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
        covered_payload_path: None,
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
            covered_payload_path: None,
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
    let file = File::open(path).ok()?;
    let mut data = Vec::with_capacity(MGL_PREFIX_BYTES);
    file.take(MGL_PREFIX_BYTES as u64)
        .read_to_end(&mut data)
        .ok()?;
    parse_mgl_metadata_xml(&String::from_utf8_lossy(&data))
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
    if let Some(region) = region_from_saturn_boot_header_file(&discovery.source_path) {
        return RegionInference {
            region: Some(region),
            confidence: "disc-header",
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
    if matches!(
        crate::library_db::path_ext(path).as_deref(),
        Some("cue" | "chd")
    ) {
        return None;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog_scan::FoundFile;
    use crate::game_discovery::{is_launcher_launch_ref, unique_discovery_count};
    use crate::launch_profiles;
    use crate::library_db::{scan_library, BenchConfig};
    use crate::sqlite_catalog::{load_arcade_catalog_from_sqlite_at, save_sqlite_scan};
    use crate::test_support::*;
    use std::time::Duration;

    #[test]
    fn saturn_region_prefers_filename_markers() {
        let root = unique_temp_dir("saturn-region-filename");
        std::fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("Nights into Dreams (USA).chd");
        let mut header = [b' '; 256];
        header[0..15].copy_from_slice(b"SEGA SEGASATURN");
        header[0x40..0x50].copy_from_slice(b"J               ");
        std::fs::write(&path, header).expect("write saturn header fixture");
        let discovery = saturn_payload(&path.display().to_string());

        assert_eq!(
            infer_region_metadata(&discovery),
            RegionInference {
                region: Some("usa"),
                confidence: "filename-high"
            }
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn saturn_region_uses_folder_when_filename_has_no_marker() {
        let discovery = saturn_payload("/media/fat/games/Saturn/Japan/Princess Crown.chd");

        assert_eq!(
            infer_region_metadata(&discovery),
            RegionInference {
                region: Some("japan"),
                confidence: "folder-medium"
            }
        );
    }

    #[test]
    fn saturn_region_stays_unknown_without_evidence() {
        let discovery = saturn_payload("/media/fat/games/Saturn/Clockwork Knight.chd");

        assert_eq!(
            infer_region_metadata(&discovery),
            RegionInference {
                region: None,
                confidence: "unknown"
            }
        );
    }

    #[test]
    fn saturn_region_skips_disc_container_boot_header_probe() {
        let root = unique_temp_dir("saturn-region-container-skip");
        std::fs::create_dir_all(&root).expect("create temp root");
        for name in ["Clockwork Knight.chd", "Clockwork Knight.cue"] {
            let path = root.join(name);
            let mut header = [b' '; 256];
            header[0..15].copy_from_slice(b"SEGA SEGASATURN");
            header[0x40..0x50].copy_from_slice(b"U               ");
            std::fs::write(&path, header).expect("write saturn header fixture");
            let discovery = saturn_payload(&path.display().to_string());

            assert_eq!(
                infer_region_metadata(&discovery),
                RegionInference {
                    region: None,
                    confidence: "unknown"
                }
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn saturn_boot_header_extracts_product_and_area() {
        let mut header = [b' '; 256];
        header[0..15].copy_from_slice(b"SEGA SEGASATURN");
        header[0x20..0x2a].copy_from_slice(b"T-12345G  ");
        header[0x40..0x50].copy_from_slice(b"JTUE            ");

        let parsed = parse_saturn_boot_header(&header).expect("saturn header");

        assert_eq!(parsed.product_id.as_deref(), Some("T-12345G"));
        assert_eq!(parsed.region, Some("usa"));
    }

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
                <category>Driving</category>
                <catver>Racing / Chase</catver>
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
        assert_eq!(metadata.category.as_deref(), Some("Driving"));
        assert_eq!(metadata.catver.as_deref(), Some("Racing / Chase"));
        assert_eq!(metadata.year.as_deref(), Some("1997"));
        assert_eq!(metadata.setname.as_deref(), Some("batcir"));
        assert_eq!(metadata.parent.as_deref(), Some("batcirj"));
        let _ = std::fs::remove_dir_all(root);
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
            size: 7,
            mtime_secs: 0,
        };
        let listing = CollectionListing {
            entry_path: "listings/games.txt",
            genre: "AmigaVision",
        };
        let start = Instant::now();

        let text =
            collection_listing_text_with_tool(&file, &listing, &helper, Duration::from_millis(75));

        assert!(text.is_none());
        assert!(start.elapsed() < Duration::from_secs(1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_listing_entries_generate_visible_collection_games() {
        let root = unique_temp_dir("amigavision-listing");
        let db = root.join("library.sqlite3");
        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "amiga")
            .expect("amiga profile");
        let listing = profile.collection_rules[0].listings[0];
        let file = FoundFile {
            path: PathBuf::from("/media/fat/games/Amiga/AmigaVision-MiSTer.7z"),
            ext: "7z".to_string(),
            size: 5_208_842_481,
            mtime_secs: 1,
        };
        let discoveries = collection_discoveries_from_listing_text(
            &file,
            profile,
            &listing,
            "Agony\nAlien Breed\n",
        );

        assert_eq!(unique_discovery_count(&discoveries), 2);
        assert!(discoveries.iter().all(|discovery| discovery
            .launch_ref
            .starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX)));
        save_sqlite_scan(&db, &sqlite_scan_with_discoveries(discoveries)).expect("save sqlite");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(loaded.rows, 2);
        assert!(loaded
            .catalog
            .games
            .iter()
            .all(|game| game.system_id.as_ref() == "amiga"));
        assert!(loaded
            .catalog
            .systems
            .iter()
            .any(|system| system.id == "amiga" && system.count == 2));
        assert!(loaded
            .catalog
            .games
            .iter()
            .all(|game| game.mra_path.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX)));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_collection_adds_launcher_entry() {
        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "amiga")
            .expect("amiga profile");
        let file = FoundFile {
            path: PathBuf::from("/media/fat/games/Amiga/AmigaVision-MiSTer.7z"),
            ext: "7z".to_string(),
            size: 5_208_842_481,
            mtime_secs: 1,
        };

        let discoveries =
            collection_discoveries_from_container(&file, profile, &profile.collection_rules[0]);

        assert!(discoveries.iter().any(|discovery| {
            discovery.title == "AmigaVision" && discovery.launch_ref == AMIGAVISION_LAUNCHER_REF
        }));
    }

    #[test]
    fn installed_amigavision_hdf_uses_launcher_and_listings() {
        let root = unique_temp_dir("amigavision-installed");
        let amiga_dir = root.join("games/Amiga");
        let listings_dir = amiga_dir.join("listings");
        std::fs::create_dir_all(&listings_dir).expect("create listings dir");
        std::fs::write(amiga_dir.join("AmigaVision.hdf"), "hdf").expect("write hdf");
        std::fs::write(amiga_dir.join("AmigaVision-Saves.hdf"), "saves").expect("write saves");
        std::fs::write(
            listings_dir.join("games.txt"),
            b"Agony (OCS)[en]\nAlien Breed (OCS)[en]\nInvalid \xff Title (OCS)[en]\n",
        )
        .expect("write games listing");
        std::fs::write(
            listings_dir.join("demos.txt"),
            "State of the Art (OCS)[demo]\n",
        )
        .expect("write demos listing");
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert!(scan.normal_files.is_empty());
        assert_eq!(scan.ignored_files, 2);
        assert!(scan.discoveries.iter().any(|discovery| {
            discovery.title == "AmigaVision" && discovery.launch_ref == AMIGAVISION_LAUNCHER_REF
        }));
        assert_eq!(
            scan.discoveries
                .iter()
                .filter(|discovery| discovery
                    .launch_ref
                    .starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX))
                .count(),
            4
        );
        assert!(scan
            .discoveries
            .iter()
            .all(|discovery| !discovery.launch_ref.ends_with("AmigaVision.hdf")));

        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(loaded.rows, 5);
        assert!(loaded.catalog.games.iter().all(|game| {
            game.mra_path.as_ref() == AMIGAVISION_LAUNCHER_REF
                || game.mra_path.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX)
        }));
        assert!(loaded
            .catalog
            .systems
            .iter()
            .any(|system| system.id == "amiga" && system.count == 5));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_archive_itself_is_not_a_launch_ref() {
        assert!(!is_launcher_launch_ref(
            "/media/fat/games/Amiga/AmigaVision-MiSTer.7z"
        ));
        assert!(is_launcher_launch_ref(AMIGAVISION_LAUNCHER_REF));
        assert!(is_launcher_launch_ref(&amigavision_game_launch_ref(
            "4th & Inches (OCS)[en]"
        )));
    }
}
