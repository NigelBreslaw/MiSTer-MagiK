// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Screenshot media identity and path helpers.

use std::fmt;
use std::path::{Path, PathBuf};

pub const DEFAULT_SCREENSHOT_ASSET_DIR: &str = "/media/fat/mister-magik/assets";

pub fn default_screenshot_asset_dir() -> PathBuf {
    std::env::var("MISTER_PREVIEW_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::device_layout::current_app_path("assets"))
}
pub const DEFAULT_SCREENSHOT_IMAGE_SIZE: &str = "320x320";
pub const SNES_SCREENSHOT_IMAGE_SIZE: &str = "256x224";
pub const ATARI_LYNX_SCREENSHOT_IMAGE_SIZE: &str = "160x102";
pub const SCREENSHOT_MEDIA_STATE_FILENAME: &str = ".screenshot-media-state.json";

/// One fixed logical raster used by every entry in a screenshot pack.
///
/// `rotatable` permits the archive to contain the width/height-swapped form
/// for systems whose games can be held in portrait orientation.  The pack
/// builder and archive reader must otherwise reject a different geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenshotResolutionProfile {
    pub width: u32,
    pub height: u32,
    pub rotatable: bool,
    pub resize_filter: &'static str,
}

impl ScreenshotResolutionProfile {
    pub const fn image_size(self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub const fn allows(self, width: u32, height: u32) -> bool {
        (width == self.width && height == self.height)
            || (self.rotatable && width == self.height && height == self.width)
    }
}

const SUPPORTED_SCREENSHOT_PACK_IDS: &[ScreenshotPackId] = &[
    ScreenshotPackId::Arcade,
    ScreenshotPackId::NeoGeo,
    ScreenshotPackId::Nes,
    ScreenshotPackId::Snes,
    ScreenshotPackId::N64,
    ScreenshotPackId::Sms,
    ScreenshotPackId::MegaDrive,
    ScreenshotPackId::Saturn,
    ScreenshotPackId::Amiga,
    ScreenshotPackId::AtariLynx,
    ScreenshotPackId::Fds,
    ScreenshotPackId::S32x,
    ScreenshotPackId::MegaCd,
    ScreenshotPackId::AmigaCd32,
    ScreenshotPackId::C64,
    ScreenshotPackId::ZxSpectrum,
    ScreenshotPackId::AcornAtom,
    ScreenshotPackId::AcornElectron,
    ScreenshotPackId::BbcMicro,
    ScreenshotPackId::Archie,
    ScreenshotPackId::AppleIi,
    ScreenshotPackId::AppleIigs,
    ScreenshotPackId::Amstrad,
    ScreenshotPackId::Atari2600,
    ScreenshotPackId::Atari5200,
    ScreenshotPackId::Atari7800,
    ScreenshotPackId::Atari800,
    ScreenshotPackId::AtariSt,
    ScreenshotPackId::C128,
    ScreenshotPackId::C16,
    ScreenshotPackId::Pet2001,
    ScreenshotPackId::Vic20,
    ScreenshotPackId::ColecoVision,
    ScreenshotPackId::MegaDuck,
    ScreenshotPackId::WonderSwan,
    ScreenshotPackId::WonderSwanColor,
    ScreenshotPackId::X68000,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenshotPackId {
    Arcade,
    NeoGeo,
    Nes,
    Snes,
    N64,
    Sms,
    MegaDrive,
    Saturn,
    Amiga,
    AtariLynx,
    Fds,
    S32x,
    MegaCd,
    AmigaCd32,
    C64,
    ZxSpectrum,
    AcornAtom,
    AcornElectron,
    BbcMicro,
    Archie,
    AppleIi,
    AppleIigs,
    Amstrad,
    Atari2600,
    Atari5200,
    Atari7800,
    Atari800,
    AtariSt,
    C128,
    C16,
    Pet2001,
    Vic20,
    ColecoVision,
    MegaDuck,
    WonderSwan,
    WonderSwanColor,
    X68000,
}

impl ScreenshotPackId {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "arcade" => Some(Self::Arcade),
            "neogeo" => Some(Self::NeoGeo),
            "nes" => Some(Self::Nes),
            "snes" => Some(Self::Snes),
            "n64" => Some(Self::N64),
            "sms" => Some(Self::Sms),
            "megadrive" => Some(Self::MegaDrive),
            "saturn" => Some(Self::Saturn),
            "amiga" => Some(Self::Amiga),
            "atarilynx" => Some(Self::AtariLynx),
            "fds" => Some(Self::Fds),
            "s32x" => Some(Self::S32x),
            "megacd" => Some(Self::MegaCd),
            "amigacd32" => Some(Self::AmigaCd32),
            "c64" => Some(Self::C64),
            "zx-spectrum" => Some(Self::ZxSpectrum),
            "acornatom" => Some(Self::AcornAtom),
            "acornelectron" => Some(Self::AcornElectron),
            "bbcmicro" => Some(Self::BbcMicro),
            "archie" => Some(Self::Archie),
            "apple-ii" => Some(Self::AppleIi),
            "apple-iigs" => Some(Self::AppleIigs),
            "amstrad" => Some(Self::Amstrad),
            "atari2600" => Some(Self::Atari2600),
            "atari5200" => Some(Self::Atari5200),
            "atari7800" => Some(Self::Atari7800),
            "atari800" => Some(Self::Atari800),
            "atarist" => Some(Self::AtariSt),
            "c128" => Some(Self::C128),
            "c16" => Some(Self::C16),
            "pet2001" => Some(Self::Pet2001),
            "vic20" => Some(Self::Vic20),
            "colecovision" => Some(Self::ColecoVision),
            "megaduck" => Some(Self::MegaDuck),
            "wonderswan" => Some(Self::WonderSwan),
            "wonderswancolor" => Some(Self::WonderSwanColor),
            "x68000" => Some(Self::X68000),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Arcade => "arcade",
            Self::NeoGeo => "neogeo",
            Self::Nes => "nes",
            Self::Snes => "snes",
            Self::N64 => "n64",
            Self::Sms => "sms",
            Self::MegaDrive => "megadrive",
            Self::Saturn => "saturn",
            Self::Amiga => "amiga",
            Self::AtariLynx => "atarilynx",
            Self::Fds => "fds",
            Self::S32x => "s32x",
            Self::MegaCd => "megacd",
            Self::AmigaCd32 => "amigacd32",
            Self::C64 => "c64",
            Self::ZxSpectrum => "zx-spectrum",
            Self::AcornAtom => "acornatom",
            Self::AcornElectron => "acornelectron",
            Self::BbcMicro => "bbcmicro",
            Self::Archie => "archie",
            Self::AppleIi => "apple-ii",
            Self::AppleIigs => "apple-iigs",
            Self::Amstrad => "amstrad",
            Self::Atari2600 => "atari2600",
            Self::Atari5200 => "atari5200",
            Self::Atari7800 => "atari7800",
            Self::Atari800 => "atari800",
            Self::AtariSt => "atarist",
            Self::C128 => "c128",
            Self::C16 => "c16",
            Self::Pet2001 => "pet2001",
            Self::Vic20 => "vic20",
            Self::ColecoVision => "colecovision",
            Self::MegaDuck => "megaduck",
            Self::WonderSwan => "wonderswan",
            Self::WonderSwanColor => "wonderswancolor",
            Self::X68000 => "x68000",
        }
    }

    pub fn supported() -> &'static [Self] {
        SUPPORTED_SCREENSHOT_PACK_IDS
    }

    pub fn legacy_filename(self) -> String {
        format!("{}-screenshots.mmlz4b", self.as_str())
    }

    pub fn size_qualified_filename(self, image_size: &ScreenshotImageSize) -> String {
        format!(
            "{}-screenshots-{}.mmlz4b",
            self.as_str(),
            image_size.as_str()
        )
    }

    pub fn legacy_path_in(self, root: &Path) -> PathBuf {
        root.join(self.legacy_filename())
    }

    pub fn size_qualified_path_in(self, root: &Path, image_size: &ScreenshotImageSize) -> PathBuf {
        root.join(self.size_qualified_filename(image_size))
    }
}

pub fn preferred_screenshot_image_size(system: &str) -> &'static str {
    match system {
        "snes" => SNES_SCREENSHOT_IMAGE_SIZE,
        "atarilynx" => ATARI_LYNX_SCREENSHOT_IMAGE_SIZE,
        "nes" | "fds" => "256x240",
        "neogeo" => "320x224",
        "n64" | "saturn" => "320x240",
        "sms" | "acornatom" | "colecovision" => "256x192",
        "megadrive" | "s32x" | "megacd" | "atari7800" => "320x224",
        "amiga" | "amiga500" | "amigacd32" => "320x200",
        "acornelectron" | "bbcmicro" | "archie" => "320x256",
        "apple-ii" => "280x192",
        "apple-iigs" => "320x200",
        "amstrad" => "320x200",
        "atari5200" => "320x192",
        "atari2600" => "160x192",
        "atari800" => "320x192",
        "atarist" | "c64" | "c128" | "c16" | "pet2001" => "320x200",
        "zx-spectrum" => "256x192",
        "vic20" => "176x184",
        "megaduck" => "160x144",
        "wonderswan" | "wonderswancolor" => "224x144",
        "x68000" => "256x256",
        _ => DEFAULT_SCREENSHOT_IMAGE_SIZE,
    }
}

/// Return the fixed logical raster and filter for a screenshot pack.
pub fn screenshot_resolution_profile(system: &str) -> Option<ScreenshotResolutionProfile> {
    let (width, height, rotatable, resize_filter) = match system {
        "arcade" => (320, 320, false, "hybrid"),
        "neogeo" => (320, 224, false, "nearest"),
        "nes" | "fds" => (256, 240, false, "nearest"),
        "snes" => (256, 224, false, "nearest"),
        "n64" | "saturn" => (320, 240, false, "hybrid"),
        "sms" | "acornatom" | "colecovision" => (256, 192, false, "nearest"),
        "megadrive" | "atari7800" => (320, 224, false, "nearest"),
        "s32x" | "megacd" => (320, 224, false, "hybrid"),
        "amiga" | "amiga500" => (320, 200, false, "nearest"),
        "amigacd32" => (320, 200, false, "hybrid"),
        "atarilynx" => (160, 102, true, "nearest"),
        "acornelectron" | "bbcmicro" | "archie" => (320, 256, false, "nearest"),
        "apple-ii" => (280, 192, false, "nearest"),
        "apple-iigs" => (320, 200, false, "nearest"),
        "amstrad" => (320, 200, false, "nearest"),
        "atari2600" => (160, 192, false, "nearest"),
        "atari5200" => (320, 192, false, "nearest"),
        "atari800" => (320, 192, false, "nearest"),
        "atarist" | "c64" | "c128" | "c16" | "pet2001" => (320, 200, false, "nearest"),
        "zx-spectrum" => (256, 192, false, "nearest"),
        "vic20" => (176, 184, false, "nearest"),
        "megaduck" => (160, 144, false, "nearest"),
        "wonderswan" | "wonderswancolor" => (224, 144, true, "nearest"),
        "x68000" => (256, 256, false, "nearest"),
        _ => return None,
    };
    Some(ScreenshotResolutionProfile {
        width,
        height,
        rotatable,
        resize_filter,
    })
}

impl fmt::Display for ScreenshotPackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScreenshotImageSize(String);

impl ScreenshotImageSize {
    pub fn parse(value: &str) -> Option<Self> {
        valid_screenshot_image_size(value).then(|| Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for ScreenshotImageSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScreenshotAssetId(String);

impl ScreenshotAssetId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn from_mame_software(list_name: &str, software_name: &str) -> Self {
        Self(format!("mame-software__{list_name}__{software_name}"))
    }

    pub fn from_amigavision_title(title: &str) -> Self {
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in title.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        Self(format!("amigavision__{hash:016x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for ScreenshotAssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn is_supported_screenshot_pack_id(id: &str) -> bool {
    ScreenshotPackId::parse(id).is_some()
}

pub fn supported_screenshot_pack_ids() -> impl Iterator<Item = &'static str> {
    ScreenshotPackId::supported().iter().map(|id| id.as_str())
}

pub fn valid_screenshot_image_size(size: &str) -> bool {
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

pub fn size_qualified_screenshot_pack_filename(
    system: &str,
    image_size: &str,
) -> Result<String, String> {
    let system = ScreenshotPackId::parse(system)
        .ok_or_else(|| format!("unsupported screenshot pack id: {system}"))?;
    let image_size = ScreenshotImageSize::parse(image_size)
        .ok_or_else(|| format!("invalid screenshot image size: {image_size}"))?;
    Ok(system.size_qualified_filename(&image_size))
}

pub fn legacy_screenshot_pack_path(root: &Path, system: &str) -> Result<PathBuf, String> {
    let system = ScreenshotPackId::parse(system)
        .ok_or_else(|| format!("unsupported screenshot pack id: {system}"))?;
    Ok(system.legacy_path_in(root))
}

pub fn size_qualified_screenshot_pack_path_in_root(
    root: &Path,
    system: &str,
    image_size: &str,
) -> Result<PathBuf, String> {
    let system = ScreenshotPackId::parse(system)
        .ok_or_else(|| format!("unsupported screenshot pack id: {system}"))?;
    let image_size = ScreenshotImageSize::parse(image_size)
        .ok_or_else(|| format!("invalid screenshot image size: {image_size}"))?;
    Ok(system.size_qualified_path_in(root, &image_size))
}

pub fn size_qualified_screenshot_pack_path(
    asset_dir: &str,
    system: &str,
    image_size: &str,
) -> Result<String, String> {
    let filename = size_qualified_screenshot_pack_filename(system, image_size)?;
    Ok(format!("{}/{}", asset_dir.trim_end_matches('/'), filename))
}

pub fn screenshot_media_state_path(asset_dir: &str) -> String {
    format!(
        "{}/{}",
        asset_dir.trim_end_matches('/'),
        SCREENSHOT_MEDIA_STATE_FILENAME
    )
}

pub fn screenshot_media_state_path_in_root(root: &Path) -> PathBuf {
    root.join(SCREENSHOT_MEDIA_STATE_FILENAME)
}

pub fn screenshot_pack_id_from_legacy_filename(name: &str) -> Option<ScreenshotPackId> {
    ScreenshotPackId::supported()
        .iter()
        .copied()
        .find(|system| name == system.legacy_filename())
}

/// Extract a pack id from either a legacy or size-qualified archive filename.
pub fn screenshot_pack_id_from_filename(name: &str) -> Option<ScreenshotPackId> {
    if let Some(id) = screenshot_pack_id_from_legacy_filename(name) {
        return Some(id);
    }
    let (system, rest) = name.split_once("-screenshots-")?;
    if !rest.ends_with(".mmlz4b") {
        return None;
    }
    let size = rest.strip_suffix(".mmlz4b")?;
    valid_screenshot_image_size(size)
        .then(|| ScreenshotPackId::parse(system))
        .flatten()
}

pub fn screenshot_reset_deletes_filename(name: &str) -> bool {
    if name == SCREENSHOT_MEDIA_STATE_FILENAME
        || name.starts_with(&format!("{SCREENSHOT_MEDIA_STATE_FILENAME}.tmp-"))
    {
        return true;
    }
    let (hidden, candidate) = name
        .strip_prefix('.')
        .map_or((false, name), |candidate| (true, candidate));
    let Some((system, rest)) = candidate.split_once("-screenshots") else {
        return false;
    };
    if ScreenshotPackId::parse(system).is_none() {
        return false;
    }
    let suffix = if let Some(suffix) = rest.strip_prefix(".mmlz4b") {
        suffix
    } else {
        let Some(rest) = rest.strip_prefix('-') else {
            return false;
        };
        let Some((image_size, suffix)) = rest.split_once(".mmlz4b") else {
            return false;
        };
        if !valid_screenshot_image_size(image_size) {
            return false;
        }
        suffix
    };
    let temporary = matches!(suffix, ".tmp" | ".idx.tmp")
        || suffix.starts_with(".tmp-")
        || suffix.starts_with(".idx.tmp-")
        || (suffix.starts_with(".bench-") && suffix.ends_with(".tmp"));
    if hidden && !temporary {
        return false;
    }
    matches!(suffix, "" | ".gz" | ".br" | ".idx") || temporary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_screenshot_image_sizes() {
        assert!(valid_screenshot_image_size("320x320"));
        assert!(valid_screenshot_image_size("160x144"));
        assert!(!valid_screenshot_image_size("320"));
        assert!(!valid_screenshot_image_size("0x320"));
        assert!(!valid_screenshot_image_size("320x"));
        assert!(!valid_screenshot_image_size("320xabc"));
    }

    #[test]
    fn native_console_packs_use_their_own_screenshot_geometry() {
        assert_eq!(preferred_screenshot_image_size("snes"), "256x224");
        assert_eq!(preferred_screenshot_image_size("atarilynx"), "160x102");
        assert_eq!(preferred_screenshot_image_size("saturn"), "320x240");
        assert_eq!(preferred_screenshot_image_size("neogeo"), "320x224");
        assert_eq!(preferred_screenshot_image_size("megaduck"), "160x144");
        assert_eq!(preferred_screenshot_image_size("c64"), "320x200");
        assert_eq!(preferred_screenshot_image_size("zx-spectrum"), "256x192");
        assert_eq!(
            ScreenshotPackId::parse("atarilynx"),
            Some(ScreenshotPackId::AtariLynx)
        );
    }

    #[test]
    fn fixed_profiles_allow_only_declared_rotated_sizes() {
        let nes = screenshot_resolution_profile("nes").unwrap();
        assert!(nes.allows(256, 240));
        assert!(!nes.allows(240, 256));
        let lynx = screenshot_resolution_profile("atarilynx").unwrap();
        assert!(lynx.allows(160, 102));
        assert!(lynx.allows(102, 160));
        assert!(!lynx.allows(320, 204));
        assert!(screenshot_pack_id_from_filename("saturn-screenshots-320x240.mmlz4b").is_some());
        assert_eq!(ScreenshotPackId::parse("c64"), Some(ScreenshotPackId::C64));
        assert_eq!(
            ScreenshotPackId::parse("zx-spectrum"),
            Some(ScreenshotPackId::ZxSpectrum)
        );
    }

    #[test]
    fn fixed_profile_registry_matches_cloud_manifest_contract() {
        let expected = [
            ("arcade", "320x320"),
            ("neogeo", "320x224"),
            ("nes", "256x240"),
            ("fds", "256x240"),
            ("snes", "256x224"),
            ("n64", "320x240"),
            ("sms", "256x192"),
            ("megadrive", "320x224"),
            ("s32x", "320x224"),
            ("megacd", "320x224"),
            ("saturn", "320x240"),
            ("amiga", "320x200"),
            ("amigacd32", "320x200"),
            ("atarilynx", "160x102"),
            ("acornatom", "256x192"),
            ("acornelectron", "320x256"),
            ("bbcmicro", "320x256"),
            ("archie", "320x256"),
            ("apple-ii", "280x192"),
            ("apple-iigs", "320x200"),
            ("amstrad", "320x200"),
            ("atari2600", "160x192"),
            ("atari5200", "320x192"),
            ("atari7800", "320x224"),
            ("atari800", "320x192"),
            ("atarist", "320x200"),
            ("c64", "320x200"),
            ("c128", "320x200"),
            ("c16", "320x200"),
            ("pet2001", "320x200"),
            ("vic20", "176x184"),
            ("colecovision", "256x192"),
            ("megaduck", "160x144"),
            ("wonderswan", "224x144"),
            ("wonderswancolor", "224x144"),
            ("x68000", "256x256"),
            ("zx-spectrum", "256x192"),
        ];
        let cloud_contract = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../private/magik-cloud/scripts/manifest_contract.py");
        if let Ok(cloud_contract) = std::fs::read_to_string(cloud_contract) {
            for &(system, size) in &expected {
                assert!(
                    screenshot_resolution_profile(system)
                        .is_some_and(
                            |profile| format!("{}x{}", profile.width, profile.height) == size
                        ),
                    "catalog profile missing or mismatched for {system}"
                );
                assert!(
                    cloud_contract.contains(&format!("\"{system}\": \"{size}\"")),
                    "cloud profile missing or mismatched for {system}"
                );
            }
        }
        let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../private/magik-cloud/manifest/v1.schema.json");
        if let Ok(schema) = std::fs::read_to_string(schema_path) {
            let schema: serde_json::Value =
                serde_json::from_str(&schema).expect("manifest schema JSON");
            let schema_ids = schema
                .pointer("/properties/packs/items/properties/id/enum")
                .and_then(serde_json::Value::as_array)
                .expect("manifest pack id enum")
                .iter()
                .map(|id| id.as_str().expect("manifest pack id string"))
                .collect::<Vec<_>>();
            let expected_ids = expected
                .iter()
                .map(|(system, _)| *system)
                .collect::<Vec<_>>();
            assert_eq!(
                schema_ids, expected_ids,
                "catalog/schema pack registry drift"
            );
        }
    }

    #[test]
    fn builds_pack_paths_and_state_paths() {
        assert_eq!(
            size_qualified_screenshot_pack_filename("arcade", "320x320").unwrap(),
            "arcade-screenshots-320x320.mmlz4b"
        );
        assert_eq!(
            size_qualified_screenshot_pack_path(DEFAULT_SCREENSHOT_ASSET_DIR, "saturn", "240x240")
                .unwrap(),
            "/media/fat/mister-magik/assets/saturn-screenshots-240x240.mmlz4b"
        );
        assert_eq!(
            screenshot_media_state_path(DEFAULT_SCREENSHOT_ASSET_DIR),
            "/media/fat/mister-magik/assets/.screenshot-media-state.json"
        );
        assert!(size_qualified_screenshot_pack_filename("psx", "320x320").is_err());
        assert!(size_qualified_screenshot_pack_filename("arcade", "large").is_err());
    }

    #[test]
    fn recognizes_legacy_pack_filenames() {
        assert_eq!(
            screenshot_pack_id_from_legacy_filename("neogeo-screenshots.mmlz4b"),
            Some(ScreenshotPackId::NeoGeo)
        );
        assert_eq!(
            screenshot_pack_id_from_legacy_filename("neogeo-screenshots-320x320.mmlz4b"),
            None
        );
    }

    #[test]
    fn reset_cleanup_matches_pack_and_state_artifacts_only() {
        assert!(screenshot_reset_deletes_filename(
            "arcade-screenshots-320x320.mmlz4b"
        ));
        assert!(screenshot_reset_deletes_filename(
            "neogeo-screenshots-240x240.mmlz4b.tmp-123"
        ));
        assert!(screenshot_reset_deletes_filename("nes-screenshots.mmlz4b"));
        assert!(screenshot_reset_deletes_filename(
            SCREENSHOT_MEDIA_STATE_FILENAME
        ));
        assert!(screenshot_reset_deletes_filename(
            ".screenshot-media-state.json.tmp-123"
        ));
        assert!(screenshot_reset_deletes_filename(
            "arcade-screenshots-320x320.mmlz4b.idx"
        ));
        assert!(screenshot_reset_deletes_filename(
            "arcade-screenshots-320x320.mmlz4b.idx.tmp-123"
        ));
        assert!(screenshot_reset_deletes_filename(
            ".arcade-screenshots-320x320.mmlz4b.tmp-123"
        ));
        assert!(!screenshot_reset_deletes_filename(
            "pcengine-screenshots.mmlz4b"
        ));
        assert!(!screenshot_reset_deletes_filename(
            "arcade-screenshots-large.mmlz4b"
        ));
        assert!(!screenshot_reset_deletes_filename(
            "arcade-preview-cache.raw565"
        ));
        assert!(!screenshot_reset_deletes_filename(
            ".arcade-screenshots-320x320.mmlz4b"
        ));
    }

    #[test]
    fn builds_console_screenshot_asset_ids() {
        assert_eq!(
            ScreenshotAssetId::from_mame_software("nes", "smb").as_str(),
            "mame-software__nes__smb"
        );
        assert_eq!(
            ScreenshotAssetId::from_amigavision_title("Alien Breed (OCS)[en]").as_str(),
            "amigavision__667cdd86c04e1709"
        );
    }
}
