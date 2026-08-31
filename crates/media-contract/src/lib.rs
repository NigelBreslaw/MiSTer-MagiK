// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use ed25519_dalek::{Signature, VerifyingKey};
use std::collections::BTreeMap;

pub const DEFAULT_MANIFEST_URL: &str =
    "https://assets.mistermagik.com/mister-magik/v1/manifest.json";
pub const OFFICIAL_ASSET_HTTPS_ORIGIN: &str = "https://assets.mistermagik.com";
pub const OFFICIAL_ASSET_HTTP_ORIGIN: &str = "http://assets.mistermagik.com";
pub const MANIFEST_SIGNATURE_SUFFIX: &str = ".sig";
pub const MANIFEST_SIGNATURE_SCHEMA: u64 = 1;
pub const MANIFEST_SIGNATURE_ALGORITHM: &str = "ed25519";
pub const MANIFEST_PRODUCTION_KEY_ID: &str = "media-prod-2026-01";
pub const MANIFEST_PRODUCTION_PUBLIC_KEY: &str =
    "fd860f0314007afd9f8ac305503a3412c4a91970dd9cdfd2fe8c0cb64f918594";
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
pub const MAX_MANIFEST_SIGNATURE_BYTES: u64 = 4 * 1024;
pub const MAX_MEDIA_PACK_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_MEDIA_INDEX_BYTES: u64 = 8 * 1024 * 1024;
pub const MEDIA_CONNECT_TIMEOUT_SECS: u64 = 10;
pub const MEDIA_TRANSFER_TIMEOUT_SECS: u64 = 20 * 60;

const TRUSTED_MANIFEST_KEYS: &[(&str, &str)] =
    &[(MANIFEST_PRODUCTION_KEY_ID, MANIFEST_PRODUCTION_PUBLIC_KEY)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestTrustMode {
    UnsignedHttps,
    SignedHttps,
}

pub const fn configured_manifest_trust_mode() -> ManifestTrustMode {
    if cfg!(feature = "signed-media-manifests") {
        ManifestTrustMode::SignedHttps
    } else {
        ManifestTrustMode::UnsignedHttps
    }
}

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

pub fn validate_https_manifest_url(manifest_url: &str) -> Result<(), String> {
    let origin = manifest_origin(manifest_url)?;
    if origin.starts_with("https://") {
        Ok(())
    } else {
        Err(format!("media manifest URL must use HTTPS: {manifest_url}"))
    }
}

pub fn manifest_signature_url(manifest_url: &str) -> Result<String, String> {
    validate_https_manifest_url(manifest_url)?;
    Ok(format!("{manifest_url}{MANIFEST_SIGNATURE_SUFFIX}"))
}

pub fn verify_manifest_signature(manifest: &[u8], envelope: &[u8]) -> Result<String, String> {
    verify_manifest_signature_with_keys(manifest, envelope, TRUSTED_MANIFEST_KEYS)
}

fn verify_manifest_signature_with_keys(
    manifest: &[u8],
    envelope: &[u8],
    trusted_keys: &[(&str, &str)],
) -> Result<String, String> {
    if manifest.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(format!("media manifest exceeds {MAX_MANIFEST_BYTES} bytes"));
    }
    if envelope.len() as u64 > MAX_MANIFEST_SIGNATURE_BYTES {
        return Err(format!(
            "media manifest signature exceeds {MAX_MANIFEST_SIGNATURE_BYTES} bytes"
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(envelope)
        .map_err(|error| format!("parse media manifest signature: {error}"))?;
    if value.get("schema").and_then(serde_json::Value::as_u64) != Some(MANIFEST_SIGNATURE_SCHEMA) {
        return Err("unsupported media manifest signature schema".to_string());
    }
    if value.get("algorithm").and_then(serde_json::Value::as_str)
        != Some(MANIFEST_SIGNATURE_ALGORITHM)
    {
        return Err("unsupported media manifest signature algorithm".to_string());
    }
    let key_id = value
        .get("key_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "media manifest signature key_id is missing".to_string())?;
    let public_key_hex = trusted_keys
        .iter()
        .find_map(|(trusted_id, key)| (*trusted_id == key_id).then_some(*key))
        .ok_or_else(|| format!("untrusted media manifest signature key: {key_id}"))?;
    let signature_hex = value
        .get("signature")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "media manifest signature is missing".to_string())?;
    let public_key = VerifyingKey::from_bytes(&hex_decode_exact::<32>(
        public_key_hex,
        "media manifest public key",
    )?)
    .map_err(|error| format!("invalid media manifest public key: {error}"))?;
    let signature = Signature::from_bytes(&hex_decode_exact::<64>(
        signature_hex,
        "media manifest signature",
    )?);
    public_key
        .verify_strict(manifest, &signature)
        .map_err(|error| format!("verify media manifest signature: {error}"))?;
    Ok(key_id.to_string())
}

fn hex_decode_exact<const N: usize>(value: &str, label: &str) -> Result<[u8; N], String> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "{label} must be {} lowercase hexadecimal characters",
            N * 2
        ));
    }
    let mut decoded = [0u8; N];
    for (index, output) in decoded.iter_mut().enumerate() {
        *output = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|error| format!("parse {label}: {error}"))?;
    }
    Ok(decoded)
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
        [
            "mister-magik",
            "v1",
            "packs",
            system,
            "screenshots",
            size,
            version,
            file,
        ] if supported(system) && fixed_image_size(system, size) && valid_component(version) => {
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
        [
            "mister-magik",
            "v1",
            "packs",
            system,
            "screenshots",
            size,
            version,
            file,
        ] if supported(system)
            && fixed_image_size(system, size)
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

fn fixed_image_size(system: &str, size: &str) -> bool {
    valid_image_size(size)
        && mister_magik_catalog::media_identity::preferred_screenshot_image_size(system) == size
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
                parsed.values.clear();
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
    use ed25519_dalek::{Signer, SigningKey};

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn signed_manifest_fixture(manifest: &[u8]) -> (String, String) {
        let signing_key = SigningKey::from_bytes(&[11u8; 32]);
        let public_key = signing_key
            .verifying_key()
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let signature = signing_key.sign(manifest);
        let signature_hex = signature
            .to_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        (
            public_key,
            format!(
                "{{\"schema\":1,\"algorithm\":\"ed25519\",\"key_id\":\"test-key\",\"signature\":\"{signature_hex}\"}}"
            ),
        )
    }

    #[test]
    fn strict_hash_rejects_length_only_tokens() {
        assert!(Sha256::parse(SHA).is_ok());
        assert!(Sha256::parse(&"z".repeat(64)).is_err());
    }

    #[test]
    fn signed_manifest_verification_covers_exact_bytes() {
        let manifest = b"{\"schema\":1}\n";
        let (public_key, envelope) = signed_manifest_fixture(manifest);
        let keys = [("test-key", public_key.as_str())];

        assert_eq!(
            verify_manifest_signature_with_keys(manifest, envelope.as_bytes(), &keys).unwrap(),
            "test-key"
        );
        assert!(
            verify_manifest_signature_with_keys(b"{\"schema\":2}\n", envelope.as_bytes(), &keys)
                .is_err()
        );
    }

    #[test]
    fn signed_manifest_verification_rejects_untrusted_and_oversized_inputs() {
        let manifest = b"{\"schema\":1}\n";
        let (public_key, envelope) = signed_manifest_fixture(manifest);
        assert!(verify_manifest_signature_with_keys(manifest, envelope.as_bytes(), &[]).is_err());
        assert!(
            verify_manifest_signature_with_keys(
                &vec![0; MAX_MANIFEST_BYTES as usize + 1],
                envelope.as_bytes(),
                &[("test-key", public_key.as_str())]
            )
            .is_err()
        );
        assert!(
            verify_manifest_signature_with_keys(
                manifest,
                &vec![0; MAX_MANIFEST_SIGNATURE_BYTES as usize + 1],
                &[("test-key", public_key.as_str())]
            )
            .is_err()
        );
    }

    #[test]
    fn manifest_urls_require_https_and_derive_signature_url() {
        assert_eq!(
            manifest_signature_url(DEFAULT_MANIFEST_URL).unwrap(),
            format!("{DEFAULT_MANIFEST_URL}.sig")
        );
        assert!(validate_https_manifest_url("http://assets.example/manifest.json").is_err());
        assert!(manifest_signature_url("assets.example/manifest.json").is_err());
    }

    #[test]
    fn configured_manifest_trust_mode_matches_compile_time_feature() {
        let expected = if cfg!(feature = "signed-media-manifests") {
            ManifestTrustMode::SignedHttps
        } else {
            ManifestTrustMode::UnsignedHttps
        };
        assert_eq!(configured_manifest_trust_mode(), expected);
    }

    #[test]
    fn object_paths_reject_traversal_and_untrusted_origin() {
        let object = format!("mister-magik/v1/packs/arcade/screenshots/320x320/v1/{SHA}.mmlz4b");
        assert!(object_url(OFFICIAL_ASSET_HTTPS_ORIGIN, &object).is_ok());
        assert!(object_url("https://evil.test", &object).is_err());
        assert!(validate_pack_object_path("../escape.mmlz4b").is_err());
        assert!(validate_pack_object_path(&object.replace("320x320", "320x224")).is_err());
        let c64 = format!("mister-magik/v1/packs/c64/screenshots/320x200/v1/{SHA}.mmlz4b");
        assert!(validate_pack_object_path(&c64).is_ok());
        assert!(validate_pack_object_path(&c64.replace("320x200", "256x192")).is_err());
    }

    #[test]
    fn headers_are_case_insensitive_and_keep_final_response_values() {
        let headers = HttpHeaders::parse(
            "HTTP/2 302\r\nLocation: /next\r\nHTTP/2 200\r\nETag: abc\r\nCF-Cache-Status: HIT\r\n",
        );
        assert_eq!(headers.status, Some(200));
        assert_eq!(headers.get("etag"), Some("abc"));
        assert_eq!(headers.get("CF-CACHE-STATUS"), Some("HIT"));
        assert_eq!(headers.get("location"), None);
    }

    #[test]
    fn compression_aliases_and_pack_variant_selection_are_normalized() {
        let raw = MediaVariant {
            compression: "none".to_string(),
            codec: "mmlz4b".to_string(),
            object: "raw".to_string(),
            bytes: 1,
            sha256: SHA.to_string(),
            url: "raw".to_string(),
        };
        let gzip = MediaVariant {
            compression: "gzip".to_string(),
            ..raw.clone()
        };
        let pack = MediaPack {
            id: "arcade".to_string(),
            version: "v1".to_string(),
            image_size: "320x320".to_string(),
            raw,
            variants: vec![gzip],
            index: None,
        };

        assert_eq!(
            pack.variant_for_compression("gz").unwrap().compression,
            "gzip"
        );
        assert!(pack.variant_for_compression("zip").is_none());
        assert_eq!(pack.identity().sha256, SHA);
    }

    #[test]
    fn hashes_origins_and_index_paths_reject_malformed_inputs() {
        assert_eq!(
            Sha256::parse(&SHA.to_ascii_uppercase()).unwrap().as_str(),
            SHA
        );
        assert!(Sha256::parse_command_output(&[0xff]).is_err());
        assert!(Sha256::parse_command_output(b"  \n").is_err());
        assert_eq!(
            Sha256::parse_command_output(format!("{SHA}  file\n").as_bytes())
                .unwrap()
                .into_string(),
            SHA
        );
        assert!(manifest_origin("assets.example/path").is_err());
        assert!(manifest_origin("https:///path").is_err());

        let index = format!("mister-magik/v1/packs/arcade/screenshots/320x320/v1/{SHA}.mmlz4b.idx");
        assert!(validate_index_object_path(&index).is_ok());
        assert!(validate_index_object_path(&format!("../{index}")).is_err());
        assert!(validate_index_object_path(&index.replace("320x320", "0x320")).is_err());
    }
}
