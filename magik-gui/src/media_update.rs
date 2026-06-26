use serde_json::Value;
use std::path::{Path, PathBuf};

pub use mister_magik_catalog::media_identity::{
    DEFAULT_SCREENSHOT_ASSET_DIR as DEFAULT_ASSET_DIR,
    DEFAULT_SCREENSHOT_IMAGE_SIZE as DEFAULT_IMAGE_SIZE,
    SCREENSHOT_MEDIA_STATE_FILENAME as STATE_FILENAME,
};

use mister_magik_catalog::media_identity::{
    is_supported_screenshot_pack_id, screenshot_media_state_path,
    size_qualified_screenshot_pack_filename, size_qualified_screenshot_pack_path,
    valid_screenshot_image_size,
};

pub const DEFAULT_MANIFEST_URL: &str =
    "https://assets.mistermagik.com/mister-magik/v1/manifest.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaManifest {
    pub schema: u64,
    pub generated_at: String,
    pub origin: String,
    pub packs: Vec<MediaPack>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaPack {
    pub id: String,
    pub version: String,
    pub image_size: String,
    pub raw: MediaVariant,
    pub variants: Vec<MediaVariant>,
    pub index: Option<MediaIndex>,
}

impl MediaPack {
    pub fn identity(&self) -> PackIdentity {
        PackIdentity {
            system: self.id.clone(),
            image_size: self.image_size.clone(),
            version: self.version.clone(),
            sha256: self.raw.sha256.clone(),
        }
    }

    pub fn variant_for_compression(&self, compression: &str) -> Option<&MediaVariant> {
        let wanted = normalize_compression(compression)?;
        self.variants
            .iter()
            .find(|variant| variant.compression == wanted)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaVariant {
    pub compression: String,
    pub codec: String,
    pub object: String,
    pub bytes: u64,
    pub sha256: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaIndex {
    pub codec: String,
    pub object: String,
    pub bytes: u64,
    pub sha256: String,
    pub url: String,
    pub archive_bytes: u64,
    pub archive_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackIdentity {
    pub system: String,
    pub image_size: String,
    pub version: String,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaUpdatePolicy {
    Off,
    Check,
    Download,
}

impl MediaUpdatePolicy {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value
            .unwrap_or("download")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "1" | "on" | "true" | "yes" | "download" => Ok(Self::Download),
            "0" | "off" | "false" | "no" => Ok(Self::Off),
            "check" | "check-only" | "dry-run" => Ok(Self::Check),
            other => Err(format!("unsupported MISTER_MEDIA_UPDATE value: {other}")),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Check => "check",
            Self::Download => "download",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalPackStatus {
    Current,
    Missing,
    Stale { reason: String },
    IndexMissing,
    IndexStale { reason: String },
}

impl LocalPackStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Missing => "missing",
            Self::Stale { .. } | Self::IndexMissing | Self::IndexStale { .. } => "stale",
        }
    }

    pub fn requires_pack_download(&self) -> bool {
        matches!(self, Self::Missing | Self::Stale { .. })
    }

    pub fn requires_index_download(&self) -> bool {
        matches!(
            self,
            Self::Missing | Self::Stale { .. } | Self::IndexMissing | Self::IndexStale { .. }
        )
    }
}

pub fn parse_manifest_json(manifest_url: &str, text: &str) -> Result<MediaManifest, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("parse media manifest json: {e}"))?;
    parse_manifest_value(manifest_url, &value)
}

pub fn parse_manifest_value(manifest_url: &str, value: &Value) -> Result<MediaManifest, String> {
    let schema = value
        .get("schema")
        .and_then(Value::as_u64)
        .ok_or("media manifest schema is missing")?;
    if schema != 1 {
        return Err(format!("unsupported media manifest schema {schema}"));
    }
    let generated_at = required_string(value, "generated_at")?.to_string();
    let origin = manifest_origin(manifest_url)?;
    let packs_value = value
        .get("packs")
        .and_then(Value::as_array)
        .ok_or("media manifest packs must be an array")?;
    let mut packs = Vec::with_capacity(packs_value.len());
    for pack_value in packs_value {
        packs.push(parse_pack(&origin, pack_value)?);
    }
    Ok(MediaManifest {
        schema,
        generated_at,
        origin,
        packs,
    })
}

pub fn manifest_origin(manifest_url: &str) -> Result<String, String> {
    let (scheme, rest) = manifest_url
        .split_once("://")
        .ok_or_else(|| format!("manifest URL must include scheme: {manifest_url}"))?;
    let host = rest
        .split('/')
        .next()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| format!("manifest URL must include host: {manifest_url}"))?;
    Ok(format!("{scheme}://{host}"))
}

pub fn is_supported_pack_id(id: &str) -> bool {
    is_supported_screenshot_pack_id(id)
}

fn parse_pack(origin: &str, value: &Value) -> Result<MediaPack, String> {
    let id = required_string(value, "id")?;
    if !is_supported_pack_id(id) {
        return Err(format!("unsupported screenshot pack id: {id}"));
    }
    let version = required_string(value, "version")?.to_string();
    let image_size = image_size_from_pack(value).unwrap_or_else(|| DEFAULT_IMAGE_SIZE.to_string());
    let raw = MediaVariant {
        compression: "none".to_string(),
        codec: required_string(value, "codec")?.to_string(),
        object: required_string(value, "object")?.to_string(),
        bytes: required_u64(value, "bytes")?,
        sha256: required_string(value, "sha256")?.to_string(),
        url: String::new(),
    }
    .with_url(origin)?;
    if raw.codec != "mmlz4b" {
        return Err(format!("pack {id} uses unsupported codec {}", raw.codec));
    }
    let variants = match value.get("variants").and_then(Value::as_array) {
        Some(items) => {
            let mut parsed = Vec::with_capacity(items.len());
            for item in items {
                parsed.push(parse_variant(origin, item)?);
            }
            if parsed.is_empty() {
                vec![raw.clone()]
            } else {
                parsed
            }
        }
        None => vec![raw.clone()],
    };
    if !variants.iter().any(|variant| variant.compression == "none") {
        return Err(format!(
            "pack {id} variants do not include compression=none"
        ));
    }
    let index = value
        .get("index")
        .map(|index| parse_index(origin, id, &raw, index))
        .transpose()?;
    Ok(MediaPack {
        id: id.to_string(),
        version,
        image_size,
        raw,
        variants,
        index,
    })
}

fn parse_variant(origin: &str, value: &Value) -> Result<MediaVariant, String> {
    let compression = normalize_compression(required_string(value, "compression")?)
        .ok_or("unsupported media variant compression")?;
    MediaVariant {
        compression: compression.to_string(),
        codec: required_string(value, "codec")?.to_string(),
        object: required_string(value, "object")?.to_string(),
        bytes: required_u64(value, "bytes")?,
        sha256: required_string(value, "sha256")?.to_string(),
        url: String::new(),
    }
    .with_url(origin)
}

fn parse_index(
    origin: &str,
    pack_id: &str,
    raw: &MediaVariant,
    value: &Value,
) -> Result<MediaIndex, String> {
    let index = MediaIndex {
        codec: required_string(value, "codec")?.to_string(),
        object: required_string(value, "object")?.to_string(),
        bytes: required_u64(value, "bytes")?,
        sha256: required_string(value, "sha256")?.to_string(),
        url: String::new(),
        archive_bytes: required_u64(value, "archive_bytes")?,
        archive_sha256: required_string(value, "archive_sha256")?.to_string(),
    }
    .with_url(origin)?;
    if index.codec != "mmlz4b-index-v2" {
        return Err(format!(
            "pack {pack_id} uses unsupported index codec {}",
            index.codec
        ));
    }
    if index.archive_bytes != raw.bytes {
        return Err(format!(
            "pack {pack_id} index archive_bytes mismatch expected={} got={}",
            raw.bytes, index.archive_bytes
        ));
    }
    if index.archive_sha256 != raw.sha256 {
        return Err(format!("pack {pack_id} index archive_sha256 mismatch"));
    }
    Ok(index)
}

fn normalize_compression(value: &str) -> Option<&'static str> {
    match value {
        "none" | "identity" => Some("none"),
        "gzip" | "gz" => Some("gzip"),
        "brotli" | "br" => Some("brotli"),
        _ => None,
    }
}

impl MediaVariant {
    fn with_url(mut self, origin: &str) -> Result<Self, String> {
        validate_object_path(&self.object)?;
        validate_sha256(&self.sha256)?;
        if self.bytes == 0 {
            return Err(format!("media object {} has zero bytes", self.object));
        }
        self.url = format!("{}/{}", origin.trim_end_matches('/'), self.object);
        Ok(self)
    }
}

impl MediaIndex {
    fn with_url(mut self, origin: &str) -> Result<Self, String> {
        validate_index_object_path(&self.object)?;
        validate_sha256(&self.sha256)?;
        validate_sha256(&self.archive_sha256)?;
        if self.bytes == 0 {
            return Err(format!("media index {} has zero bytes", self.object));
        }
        if self.archive_bytes == 0 {
            return Err(format!(
                "media index {} has zero archive bytes",
                self.object
            ));
        }
        self.url = format!("{}/{}", origin.trim_end_matches('/'), self.object);
        Ok(self)
    }
}

fn image_size_from_pack(value: &Value) -> Option<String> {
    for key in [
        "image_size",
        "preview_size",
        "pack_size",
        "size",
        "resolution",
    ] {
        if let Some(size) = value.get(key).and_then(Value::as_str) {
            if valid_image_size(size) {
                return Some(size.to_string());
            }
        }
    }
    let width = value
        .get("image_width")
        .or_else(|| value.get("width"))
        .and_then(Value::as_u64)?;
    let height = value
        .get("image_height")
        .or_else(|| value.get("height"))
        .and_then(Value::as_u64)?;
    let size = format!("{width}x{height}");
    valid_image_size(&size).then_some(size)
}

pub fn valid_image_size(size: &str) -> bool {
    valid_screenshot_image_size(size)
}

pub fn size_qualified_pack_filename(system: &str, image_size: &str) -> Result<String, String> {
    size_qualified_screenshot_pack_filename(system, image_size)
}

pub fn size_qualified_pack_path(
    asset_dir: &str,
    system: &str,
    image_size: &str,
) -> Result<String, String> {
    size_qualified_screenshot_pack_path(asset_dir, system, image_size)
}

pub fn state_path(asset_dir: &str) -> String {
    screenshot_media_state_path(asset_dir)
}

pub fn index_path_for_pack_path(pack_path: &Path) -> PathBuf {
    let mut path = pack_path.as_os_str().to_os_string();
    path.push(".idx");
    PathBuf::from(path)
}

pub fn pack_status_from_state(
    pack: &MediaPack,
    local_path: &Path,
    state: Option<&Value>,
) -> LocalPackStatus {
    if !local_path.exists() {
        return LocalPackStatus::Missing;
    }
    let Some(entry) = state_entry_for_pack(state, pack) else {
        return LocalPackStatus::Stale {
            reason: "state-missing".to_string(),
        };
    };
    for (key, expected) in [
        ("version", pack.version.as_str()),
        ("sha256", pack.raw.sha256.as_str()),
        ("image_size", pack.image_size.as_str()),
    ] {
        match entry.get(key).and_then(Value::as_str) {
            Some(got) if got == expected => {}
            Some(got) => {
                return LocalPackStatus::Stale {
                    reason: format!("{key}-mismatch:{got}"),
                };
            }
            None if key == "image_size" => {}
            None => {
                return LocalPackStatus::Stale {
                    reason: format!("{key}-missing"),
                };
            }
        }
    }
    if let Some(index) = &pack.index {
        let index_path = index_path_for_pack_path(local_path);
        if !index_path.exists() {
            return LocalPackStatus::IndexMissing;
        }
        let Some(index_state) = entry.get("index").and_then(Value::as_object) else {
            return LocalPackStatus::IndexStale {
                reason: "index-state-missing".to_string(),
            };
        };
        for (key, expected) in [
            ("codec", index.codec.as_str()),
            ("object", index.object.as_str()),
            ("sha256", index.sha256.as_str()),
            ("archive_sha256", index.archive_sha256.as_str()),
        ] {
            match index_state.get(key).and_then(Value::as_str) {
                Some(got) if got == expected => {}
                Some(got) => {
                    return LocalPackStatus::IndexStale {
                        reason: format!("{key}-mismatch:{got}"),
                    };
                }
                None => {
                    return LocalPackStatus::IndexStale {
                        reason: format!("{key}-missing"),
                    };
                }
            }
        }
        for (key, expected) in [
            ("bytes", index.bytes),
            ("archive_bytes", index.archive_bytes),
        ] {
            match index_state.get(key).and_then(Value::as_u64) {
                Some(got) if got == expected => {}
                Some(got) => {
                    return LocalPackStatus::IndexStale {
                        reason: format!("{key}-mismatch:{got}"),
                    };
                }
                None => {
                    return LocalPackStatus::IndexStale {
                        reason: format!("{key}-missing"),
                    };
                }
            }
        }
    }
    LocalPackStatus::Current
}

fn state_entry_for_pack<'a>(state: Option<&'a Value>, pack: &MediaPack) -> Option<&'a Value> {
    let state = state?;
    let system = state.get("systems")?.get(&pack.id)?;
    system
        .get("packs")
        .and_then(|packs| packs.get(&pack.image_size))
        .or(Some(system))
}

fn validate_object_path(object: &str) -> Result<(), String> {
    if object.contains("..") || object.starts_with('/') {
        return Err(format!("unsafe media object path: {object}"));
    }
    let parts: Vec<_> = object.split('/').collect();
    if parts.len() < 6
        || parts[0] != "mister-magik"
        || parts[1] != "v1"
        || parts[2] != "packs"
        || !is_supported_pack_id(parts[3])
    {
        return Err(format!("unexpected media object path: {object}"));
    }
    match parts.as_slice() {
        // Compatibility path used by early manifests:
        // mister-magik/v1/packs/<system>/<version>/<sha>.mmlz4b
        ["mister-magik", "v1", "packs", system, version, _file]
            if is_supported_pack_id(system) && valid_version_component(version) => {}
        // Current magik-cloud path:
        // mister-magik/v1/packs/<system>/screenshots/<size>/<version>/<sha>.mmlz4b
        ["mister-magik", "v1", "packs", system, "screenshots", size, version, _file]
            if is_supported_pack_id(system)
                && valid_image_size(size)
                && valid_version_component(version) => {}
        _ => return Err(format!("unexpected media object path: {object}")),
    }
    let file = parts.last().copied().unwrap_or("");
    let valid_ext =
        file.ends_with(".mmlz4b") || file.ends_with(".mmlz4b.gz") || file.ends_with(".mmlz4b.br");
    if !valid_ext {
        return Err(format!("unexpected media object extension: {object}"));
    }
    let sha = file.split('.').next().unwrap_or("");
    validate_sha256(sha)
}

fn validate_index_object_path(object: &str) -> Result<(), String> {
    if object.contains("..") || object.starts_with('/') {
        return Err(format!("unsafe media index object path: {object}"));
    }
    let parts: Vec<_> = object.split('/').collect();
    match parts.as_slice() {
        ["mister-magik", "v1", "packs", system, "screenshots", size, version, _file]
            if is_supported_pack_id(system)
                && valid_image_size(size)
                && valid_version_component(version) => {}
        _ => return Err(format!("unexpected media index object path: {object}")),
    }
    let file = parts.last().copied().unwrap_or("");
    if !file.ends_with(".mmlz4b.idx") {
        return Err(format!("unexpected media index object extension: {object}"));
    }
    let sha = file.split('.').next().unwrap_or("");
    validate_sha256(sha)
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(format!("invalid sha256: {value}"))
    }
}

fn valid_version_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("media manifest field {key} must be a string"))
}

fn required_u64(value: &Value, key: &str) -> Result<u64, String> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("media manifest field {key} must be an integer"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const GZ_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const IDX_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn raw_manifest(extra: &str) -> String {
        format!(
            r#"{{
  "schema": 1,
  "generated_at": "2026-06-22T16:58:28Z",
  "packs": [
    {{
      "id": "arcade",
      "version": "2026.06.22",
      {extra}
      "object": "mister-magik/v1/packs/arcade/screenshots/320x320/2026.06.22/{SHA}.mmlz4b",
      "bytes": 123,
      "sha256": "{SHA}",
      "codec": "mmlz4b"
    }}
  ]
}}"#
        )
    }

    fn indexed_manifest() -> String {
        raw_manifest(&format!(
            r#""index": {{
        "object": "mister-magik/v1/packs/arcade/screenshots/320x320/2026.06.22/{IDX_SHA}.mmlz4b.idx",
        "bytes": 456,
        "sha256": "{IDX_SHA}",
        "codec": "mmlz4b-index-v2",
        "archive_bytes": 123,
        "archive_sha256": "{SHA}"
      }},"#
        ))
    }

    #[test]
    fn parses_raw_only_manifest_with_default_size() {
        let manifest = parse_manifest_json(DEFAULT_MANIFEST_URL, &raw_manifest("")).unwrap();
        let pack = &manifest.packs[0];

        assert_eq!(pack.id, "arcade");
        assert_eq!(pack.image_size, "320x320");
        assert_eq!(pack.raw.url, format!("https://assets.mistermagik.com/mister-magik/v1/packs/arcade/screenshots/320x320/2026.06.22/{SHA}.mmlz4b"));
        assert_eq!(
            pack.identity(),
            PackIdentity {
                system: "arcade".to_string(),
                image_size: "320x320".to_string(),
                version: "2026.06.22".to_string(),
                sha256: SHA.to_string(),
            }
        );
        assert_eq!(pack.variants.len(), 1);
        assert_eq!(pack.variants[0].compression, "none");
    }

    #[test]
    fn parses_manifest_index_sidecar() {
        let manifest = parse_manifest_json(DEFAULT_MANIFEST_URL, &indexed_manifest()).unwrap();
        let pack = &manifest.packs[0];
        let index = pack.index.as_ref().expect("index sidecar");

        assert_eq!(index.codec, "mmlz4b-index-v2");
        assert_eq!(index.bytes, 456);
        assert_eq!(index.archive_bytes, pack.raw.bytes);
        assert_eq!(index.archive_sha256, pack.raw.sha256);
        assert_eq!(
            index.url,
            format!("https://assets.mistermagik.com/mister-magik/v1/packs/arcade/screenshots/320x320/2026.06.22/{IDX_SHA}.mmlz4b.idx")
        );
    }

    #[test]
    fn rejects_index_sidecar_for_different_archive() {
        let text = indexed_manifest().replace(
            &format!(r#""archive_sha256": "{SHA}""#),
            r#""archive_sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc""#,
        );

        assert!(parse_manifest_json(DEFAULT_MANIFEST_URL, &text)
            .unwrap_err()
            .contains("archive_sha256"));
    }

    #[test]
    fn parses_variant_aware_manifest_and_size_field() {
        let text = format!(
            r#"{{
  "schema": 1,
  "generated_at": "2026-06-22T16:58:28Z",
  "packs": [
    {{
      "id": "neogeo",
      "version": "2026.06.22",
      "image_size": "240x240",
      "object": "mister-magik/v1/packs/neogeo/screenshots/240x240/2026.06.22/{SHA}.mmlz4b",
      "bytes": 123,
      "sha256": "{SHA}",
      "codec": "mmlz4b",
      "variants": [
        {{
          "compression": "none",
          "codec": "mmlz4b",
          "object": "mister-magik/v1/packs/neogeo/screenshots/240x240/2026.06.22/{SHA}.mmlz4b",
          "bytes": 123,
          "sha256": "{SHA}"
        }},
        {{
          "compression": "gzip",
          "codec": "mmlz4b+gzip",
          "object": "mister-magik/v1/packs/neogeo/screenshots/240x240/2026.06.22/{GZ_SHA}.mmlz4b.gz",
          "bytes": 90,
          "sha256": "{GZ_SHA}"
        }}
      ]
    }}
  ]
}}"#
        );
        let manifest = parse_manifest_json(DEFAULT_MANIFEST_URL, &text).unwrap();
        let pack = &manifest.packs[0];

        assert_eq!(pack.image_size, "240x240");
        assert_eq!(
            pack.variant_for_compression("identity")
                .unwrap()
                .compression,
            "none"
        );
        assert_eq!(
            pack.variant_for_compression("gzip").unwrap().url,
            format!("https://assets.mistermagik.com/mister-magik/v1/packs/neogeo/screenshots/240x240/2026.06.22/{GZ_SHA}.mmlz4b.gz")
        );
    }

    #[test]
    fn parses_width_and_height_size_fields() {
        let manifest = parse_manifest_json(
            DEFAULT_MANIFEST_URL,
            &raw_manifest(r#""width": 160, "height": 144,"#),
        )
        .unwrap();

        assert_eq!(manifest.packs[0].image_size, "160x144");
    }

    #[test]
    fn builds_size_qualified_pack_paths() {
        assert_eq!(
            size_qualified_pack_filename("arcade", "320x320").unwrap(),
            "arcade-screenshots-320x320.mmlz4b"
        );
        assert_eq!(
            size_qualified_pack_path(DEFAULT_ASSET_DIR, "saturn", "240x240").unwrap(),
            "/media/fat/mister-magik/assets/saturn-screenshots-240x240.mmlz4b"
        );
        assert!(size_qualified_pack_filename("psx", "320x320").is_err());
        assert!(size_qualified_pack_filename("arcade", "320").is_err());
    }

    #[test]
    fn classifies_pack_status_from_state_and_file() {
        let manifest = parse_manifest_json(DEFAULT_MANIFEST_URL, &raw_manifest("")).unwrap();
        let pack = &manifest.packs[0];
        let root =
            std::env::temp_dir().join(format!("mister-magik-media-state-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let local_path = root.join("arcade-screenshots-320x320.mmlz4b");

        assert_eq!(
            pack_status_from_state(pack, &local_path, None),
            LocalPackStatus::Missing
        );

        std::fs::write(&local_path, b"pack").unwrap();
        assert_eq!(
            pack_status_from_state(pack, &local_path, None),
            LocalPackStatus::Stale {
                reason: "state-missing".to_string()
            }
        );

        let state = serde_json::json!({
            "systems": {
                "arcade": {
                    "preferred_size": "320x320",
                    "packs": {
                        "320x320": {
                            "version": "2026.06.22",
                            "image_size": "320x320",
                            "sha256": SHA
                        }
                    }
                }
            }
        });
        assert_eq!(
            pack_status_from_state(pack, &local_path, Some(&state)),
            LocalPackStatus::Current
        );
        std::fs::remove_file(&local_path).unwrap();
        assert_eq!(
            pack_status_from_state(pack, &local_path, Some(&state)),
            LocalPackStatus::Missing
        );
        std::fs::write(&local_path, b"pack").unwrap();

        let stale = serde_json::json!({
            "systems": {
                "arcade": {
                    "packs": {
                        "320x320": {
                            "version": "2026.06.21",
                            "image_size": "320x320",
                            "sha256": SHA
                        }
                    }
                }
            }
        });
        assert!(matches!(
            pack_status_from_state(pack, &local_path, Some(&stale)),
            LocalPackStatus::Stale { .. }
        ));
        let _ = std::fs::remove_file(local_path);
        let _ = std::fs::remove_dir(root);
    }

    #[test]
    fn classifies_index_status_from_state_and_file() {
        let manifest = parse_manifest_json(DEFAULT_MANIFEST_URL, &indexed_manifest()).unwrap();
        let pack = &manifest.packs[0];
        let root = std::env::temp_dir().join(format!(
            "mister-magik-media-index-state-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let local_path = root.join("arcade-screenshots-320x320.mmlz4b");
        let index_path = index_path_for_pack_path(&local_path);
        std::fs::write(&local_path, b"pack").unwrap();

        let state = serde_json::json!({
            "systems": {
                "arcade": {
                    "packs": {
                        "320x320": {
                            "version": "2026.06.22",
                            "image_size": "320x320",
                            "sha256": SHA,
                            "index": {
                                "codec": "mmlz4b-index-v2",
                                "object": format!("mister-magik/v1/packs/arcade/screenshots/320x320/2026.06.22/{IDX_SHA}.mmlz4b.idx"),
                                "bytes": 456,
                                "sha256": IDX_SHA,
                                "archive_bytes": 123,
                                "archive_sha256": SHA
                            }
                        }
                    }
                }
            }
        });
        assert_eq!(
            pack_status_from_state(pack, &local_path, Some(&state)),
            LocalPackStatus::IndexMissing
        );

        std::fs::write(&index_path, b"idx").unwrap();
        assert_eq!(
            pack_status_from_state(pack, &local_path, Some(&state)),
            LocalPackStatus::Current
        );

        let stale = serde_json::json!({
            "systems": {
                "arcade": {
                    "packs": {
                        "320x320": {
                            "version": "2026.06.22",
                            "image_size": "320x320",
                            "sha256": SHA,
                            "index": {
                                "codec": "mmlz4b-index-v2",
                                "object": format!("mister-magik/v1/packs/arcade/screenshots/320x320/2026.06.22/{IDX_SHA}.mmlz4b.idx"),
                                "bytes": 1,
                                "sha256": IDX_SHA,
                                "archive_bytes": 123,
                                "archive_sha256": SHA
                            }
                        }
                    }
                }
            }
        });
        assert!(matches!(
            pack_status_from_state(pack, &local_path, Some(&stale)),
            LocalPackStatus::IndexStale { .. }
        ));
        let _ = std::fs::remove_file(local_path);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_dir(root);
    }

    #[test]
    fn rejects_unsupported_pack_id() {
        let text = raw_manifest("").replace(r#""id": "arcade""#, r#""id": "psx""#);

        assert!(parse_manifest_json(DEFAULT_MANIFEST_URL, &text)
            .unwrap_err()
            .contains("unsupported"));
    }

    #[test]
    fn rejects_unsafe_object_path() {
        let text = raw_manifest("").replace(
            "mister-magik/v1/packs/arcade/screenshots/320x320/2026.06.22/",
            "../packs/arcade/screenshots/320x320/2026.06.22/",
        );

        assert!(parse_manifest_json(DEFAULT_MANIFEST_URL, &text)
            .unwrap_err()
            .contains("unsafe"));
    }

    #[test]
    fn parses_compatibility_manifest_object_paths() {
        let text = raw_manifest("").replace(
            "mister-magik/v1/packs/arcade/screenshots/320x320/2026.06.22/",
            "mister-magik/v1/packs/arcade/2026.06.22/",
        );
        let manifest = parse_manifest_json(DEFAULT_MANIFEST_URL, &text).unwrap();

        assert_eq!(
            manifest.packs[0].raw.url,
            format!(
                "https://assets.mistermagik.com/mister-magik/v1/packs/arcade/2026.06.22/{SHA}.mmlz4b"
            )
        );
    }

    #[test]
    fn rejects_unexpected_artifact_object_path() {
        let text = raw_manifest("").replace(
            "mister-magik/v1/packs/arcade/screenshots/320x320/2026.06.22/",
            "mister-magik/v1/packs/arcade/covers/320x320/2026.06.22/",
        );

        assert!(parse_manifest_json(DEFAULT_MANIFEST_URL, &text)
            .unwrap_err()
            .contains("unexpected"));
    }
}
