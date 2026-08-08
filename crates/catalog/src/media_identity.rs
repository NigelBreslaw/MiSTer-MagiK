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
        _ => DEFAULT_SCREENSHOT_IMAGE_SIZE,
    }
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
        assert_eq!(preferred_screenshot_image_size("saturn"), "320x320");
        assert_eq!(
            ScreenshotPackId::parse("atarilynx"),
            Some(ScreenshotPackId::AtariLynx)
        );
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
