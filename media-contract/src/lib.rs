// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeMap;

pub const DEFAULT_MANIFEST_URL: &str =
    "https://assets.mistermagik.com/mister-magik/v1/manifest.json";
pub const OFFICIAL_ASSET_HTTPS_ORIGIN: &str = "https://assets.mistermagik.com";
pub const OFFICIAL_ASSET_HTTP_ORIGIN: &str = "http://assets.mistermagik.com";

pub fn normalize_compression(value: &str) -> Option<&'static str> {
    match value {
        "none" | "identity" => Some("none"),
        "gzip" | "gz" => Some("gzip"),
        "brotli" | "br" => Some("brotli"),
        _ => None,
    }
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sha256(String);

impl Sha256 {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            Ok(Self(value.to_ascii_lowercase()))
        } else {
            Err(format!("invalid sha256: {value}"))
        }
    }

    pub fn parse_command_output(output: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(output).map_err(|error| format!("sha256 utf8: {error}"))?;
        let value = text
            .split_whitespace()
            .next()
            .ok_or_else(|| format!("could not parse sha256 output: {text}"))?;
        Self::parse(value).map_err(|_| format!("could not parse sha256 output: {text}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
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

pub fn validate_manifest_origin(origin: &str) -> Result<(), String> {
    if matches!(
        origin,
        OFFICIAL_ASSET_HTTPS_ORIGIN | OFFICIAL_ASSET_HTTP_ORIGIN
    ) {
        Ok(())
    } else {
        Err(format!("unsupported media manifest origin: {origin}"))
    }
}

pub fn object_url(origin: &str, object: &str) -> Result<String, String> {
    validate_manifest_origin(origin)?;
    validate_pack_object_path(object)?;
    Ok(format!("{}/{}", origin.trim_end_matches('/'), object))
}

pub fn validate_pack_object_path(object: &str) -> Result<(), String> {
    if object.contains("..") || object.starts_with('/') {
        return Err(format!("unsafe media object path: {object}"));
    }
    let parts: Vec<_> = object.split('/').collect();
    match parts.as_slice() {
        ["mister-magik", "v1", "packs", system, version, file]
            if supported(system) && valid_component(version) =>
        {
            validate_pack_filename(file)
        }
        ["mister-magik", "v1", "packs", system, "screenshots", size, version, file]
            if supported(system) && valid_image_size(size) && valid_component(version) =>
        {
            validate_pack_filename(file)
        }
        _ => Err(format!("unexpected media object path: {object}")),
    }
}

pub fn validate_index_object_path(object: &str) -> Result<(), String> {
    if object.contains("..") || object.starts_with('/') {
        return Err(format!("unsafe media index object path: {object}"));
    }
    let parts: Vec<_> = object.split('/').collect();
    match parts.as_slice() {
        ["mister-magik", "v1", "packs", system, "screenshots", size, version, file]
            if supported(system)
                && valid_image_size(size)
                && valid_component(version)
                && file.ends_with(".mmlz4b.idx") =>
        {
            Sha256::parse(file.split('.').next().unwrap_or(""))?;
            Ok(())
        }
        _ => Err(format!("unexpected media index object path: {object}")),
    }
}

fn validate_pack_filename(file: &str) -> Result<(), String> {
    if !(file.ends_with(".mmlz4b") || file.ends_with(".mmlz4b.gz") || file.ends_with(".mmlz4b.br"))
    {
        return Err(format!("unexpected media object extension: {file}"));
    }
    Sha256::parse(file.split('.').next().unwrap_or(""))?;
    Ok(())
}

fn supported(system: &str) -> bool {
    mister_magik_catalog::media_identity::is_supported_screenshot_pack_id(system)
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
}

fn valid_image_size(value: &str) -> bool {
    value
        .split_once('x')
        .and_then(|(width, height)| Some((width.parse::<u32>().ok()?, height.parse::<u32>().ok()?)))
        .is_some_and(|(width, height)| width > 0 && height > 0)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpHeaders {
    values: BTreeMap<String, String>,
    pub status: Option<u16>,
}

impl HttpHeaders {
    pub fn parse(text: &str) -> Self {
        let mut parsed = Self::default();
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if let Some(rest) = line.strip_prefix("HTTP/") {
                parsed.status = rest
                    .split_whitespace()
                    .nth(1)
                    .and_then(|value| value.parse().ok());
                continue;
            }
            if let Some((name, value)) = line.split_once(':') {
                parsed.values.insert(
                    name.trim().to_ascii_lowercase(),
                    value.trim().trim_end_matches('\r').to_string(),
                );
            }
        }
        parsed
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn strict_hash_rejects_length_only_tokens() {
        assert!(Sha256::parse(SHA).is_ok());
        assert!(Sha256::parse(&"z".repeat(64)).is_err());
    }

    #[test]
    fn object_paths_reject_traversal_and_untrusted_origin() {
        let object = format!("mister-magik/v1/packs/arcade/screenshots/320x320/v1/{SHA}.mmlz4b");
        assert!(object_url(OFFICIAL_ASSET_HTTPS_ORIGIN, &object).is_ok());
        assert!(object_url("https://evil.test", &object).is_err());
        assert!(validate_pack_object_path("../escape.mmlz4b").is_err());
    }

    #[test]
    fn headers_are_case_insensitive_and_keep_final_response_values() {
        let headers = HttpHeaders::parse(
            "HTTP/2 302\r\nLocation: /next\r\nHTTP/2 200\r\nETag: abc\r\nCF-Cache-Status: HIT\r\n",
        );
        assert_eq!(headers.status, Some(200));
        assert_eq!(headers.get("etag"), Some("abc"));
        assert_eq!(headers.get("CF-CACHE-STATUS"), Some("HIT"));
    }
}
