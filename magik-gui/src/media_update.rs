use serde_json::Value;

pub const DEFAULT_MANIFEST_URL: &str =
    "https://assets.mistermagik.com/mister-magik/v1/manifest.json";
pub const DEFAULT_IMAGE_SIZE: &str = "320x320";
pub const DEFAULT_ASSET_DIR: &str = "/media/fat/mister-magik/assets";
pub const STATE_FILENAME: &str = ".screenshot-media-state.json";

const SUPPORTED_PACK_IDS: &[&str] = &[
    "arcade",
    "neogeo",
    "nes",
    "snes",
    "n64",
    "sms",
    "megadrive",
    "saturn",
];

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
pub struct PackIdentity {
    pub system: String,
    pub image_size: String,
    pub version: String,
    pub sha256: String,
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
    SUPPORTED_PACK_IDS.contains(&id)
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
    Ok(MediaPack {
        id: id.to_string(),
        version,
        image_size,
        raw,
        variants,
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
    let Some((w, h)) = size.split_once('x') else {
        return false;
    };
    !w.is_empty()
        && !h.is_empty()
        && w.chars().all(|ch| ch.is_ascii_digit())
        && h.chars().all(|ch| ch.is_ascii_digit())
        && w.parse::<u32>().is_ok_and(|value| value > 0)
        && h.parse::<u32>().is_ok_and(|value| value > 0)
}

pub fn size_qualified_pack_filename(system: &str, image_size: &str) -> Result<String, String> {
    if !is_supported_pack_id(system) {
        return Err(format!("unsupported screenshot pack id: {system}"));
    }
    if !valid_image_size(image_size) {
        return Err(format!("invalid screenshot image size: {image_size}"));
    }
    Ok(format!("{system}-screenshots-{image_size}.mmlz4b"))
}

pub fn size_qualified_pack_path(
    asset_dir: &str,
    system: &str,
    image_size: &str,
) -> Result<String, String> {
    let filename = size_qualified_pack_filename(system, image_size)?;
    Ok(format!("{}/{}", asset_dir.trim_end_matches('/'), filename))
}

fn validate_object_path(object: &str) -> Result<(), String> {
    if object.contains("..") || object.starts_with('/') {
        return Err(format!("unsafe media object path: {object}"));
    }
    let parts: Vec<_> = object.split('/').collect();
    if parts.len() != 6
        || parts[0] != "mister-magik"
        || parts[1] != "v1"
        || parts[2] != "packs"
        || !is_supported_pack_id(parts[3])
    {
        return Err(format!("unexpected media object path: {object}"));
    }
    let file = parts[5];
    let valid_ext =
        file.ends_with(".mmlz4b") || file.ends_with(".mmlz4b.gz") || file.ends_with(".mmlz4b.br");
    if !valid_ext {
        return Err(format!("unexpected media object extension: {object}"));
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
      "object": "mister-magik/v1/packs/arcade/2026.06.22/{SHA}.mmlz4b",
      "bytes": 123,
      "sha256": "{SHA}",
      "codec": "mmlz4b"
    }}
  ]
}}"#
        )
    }

    #[test]
    fn parses_raw_only_manifest_with_default_size() {
        let manifest = parse_manifest_json(DEFAULT_MANIFEST_URL, &raw_manifest("")).unwrap();
        let pack = &manifest.packs[0];

        assert_eq!(pack.id, "arcade");
        assert_eq!(pack.image_size, "320x320");
        assert_eq!(pack.raw.url, format!("https://assets.mistermagik.com/mister-magik/v1/packs/arcade/2026.06.22/{SHA}.mmlz4b"));
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
      "object": "mister-magik/v1/packs/neogeo/2026.06.22/{SHA}.mmlz4b",
      "bytes": 123,
      "sha256": "{SHA}",
      "codec": "mmlz4b",
      "variants": [
        {{
          "compression": "none",
          "codec": "mmlz4b",
          "object": "mister-magik/v1/packs/neogeo/2026.06.22/{SHA}.mmlz4b",
          "bytes": 123,
          "sha256": "{SHA}"
        }},
        {{
          "compression": "gzip",
          "codec": "mmlz4b+gzip",
          "object": "mister-magik/v1/packs/neogeo/2026.06.22/{GZ_SHA}.mmlz4b.gz",
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
            format!("https://assets.mistermagik.com/mister-magik/v1/packs/neogeo/2026.06.22/{GZ_SHA}.mmlz4b.gz")
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
    fn rejects_unsupported_pack_id() {
        let text = raw_manifest("").replace(r#""id": "arcade""#, r#""id": "psx""#);

        assert!(parse_manifest_json(DEFAULT_MANIFEST_URL, &text)
            .unwrap_err()
            .contains("unsupported"));
    }

    #[test]
    fn rejects_unsafe_object_path() {
        let text = raw_manifest("").replace(
            "mister-magik/v1/packs/arcade/2026.06.22/",
            "../packs/arcade/2026.06.22/",
        );

        assert!(parse_manifest_json(DEFAULT_MANIFEST_URL, &text)
            .unwrap_err()
            .contains("unsafe"));
    }
}
