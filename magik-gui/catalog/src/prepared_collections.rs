//! Shared metadata for collections that provide their own one-click launch artifacts.

use std::fmt;
use std::path::Path;

use crate::media_metadata::{inspect_mgl, resolve_mgl_payload_path, MglInspection};

pub const PREPARED_COLLECTION_ADAPTER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedCollectionId {
    AmigaVision,
    ZeroMhz,
    Neon68k,
    OneLoad64,
}

impl PreparedCollectionId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AmigaVision => "amigavision",
            Self::ZeroMhz => "0mhz",
            Self::Neon68k => "neon68k",
            Self::OneLoad64 => "oneload64",
        }
    }
}

impl fmt::Display for PreparedCollectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchQuality {
    Prepared,
    Generic,
}

impl LaunchQuality {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Generic => "generic",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedLaunchProvenance {
    pub collection_id: PreparedCollectionId,
    pub launch_quality: LaunchQuality,
    pub adapter_version: u32,
}

impl PreparedLaunchProvenance {
    pub const fn prepared(collection_id: PreparedCollectionId) -> Self {
        Self {
            collection_id,
            launch_quality: LaunchQuality::Prepared,
            adapter_version: PREPARED_COLLECTION_ADAPTER_VERSION,
        }
    }
}

pub(crate) fn validate_0mhz_mgl(path: &Path) -> Result<MglInspection, String> {
    let inspection = inspect_mgl(path)?;
    let rbf = inspection
        .rbf
        .as_deref()
        .ok_or_else(|| "0MHz MGL has no RBF".to_string())?;
    if crate::library_db::normalize_id(rbf) != "ao486" {
        return Err(format!("0MHz MGL targets {rbf}, expected AO486"));
    }
    if inspection.files.is_empty() {
        return Err("0MHz MGL has no file mount actions".to_string());
    }
    if inspection.reset_count == 0 {
        return Err("0MHz MGL has no reset action".to_string());
    }
    for action in &inspection.files {
        let payload = resolve_mgl_payload_path(path, &action.path);
        if !payload.is_file() {
            return Err(format!("0MHz MGL payload is missing: {}", payload.display()));
        }
    }
    Ok(inspection)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mister-magik-prepared-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        dir
    }

    #[test]
    fn prepared_provenance_uses_stable_storage_values() {
        let provenance = PreparedLaunchProvenance::prepared(PreparedCollectionId::ZeroMhz);
        assert_eq!(provenance.collection_id.as_str(), "0mhz");
        assert_eq!(provenance.launch_quality.as_str(), "prepared");
        assert_eq!(
            provenance.adapter_version,
            PREPARED_COLLECTION_ADAPTER_VERSION
        );
    }

    #[test]
    fn zero_mhz_validation_accepts_vhd_and_multi_image_launchers() {
        let dir = fixture_dir("0mhz-valid");
        std::fs::write(dir.join("doom.vhd"), b"vhd").expect("write vhd");
        std::fs::write(dir.join("disc.chd"), b"chd").expect("write chd");
        let mgl = dir.join("Doom.mgl");
        std::fs::write(
            &mgl,
            r#"<mistergamedescription>
                <rbf>AO486</rbf>
                <file delay="1" type="s" index="2" path="doom.vhd"/>
                <file delay="1" type="s" index="4">disc.chd</file>
                <reset delay="1"/>
            </mistergamedescription>"#,
        )
        .expect("write mgl");

        let inspection = validate_0mhz_mgl(&mgl).expect("validate 0MHz MGL");

        assert_eq!(inspection.files.len(), 2);
        assert_eq!(inspection.files[0].index, Some(2));
        assert_eq!(inspection.files[1].path, "disc.chd");
        assert_eq!(inspection.reset_count, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zero_mhz_validation_rejects_wrong_core_missing_payload_and_reset() {
        let dir = fixture_dir("0mhz-invalid");
        let wrong_core = dir.join("wrong-core.mgl");
        std::fs::write(
            &wrong_core,
            r#"<mistergamedescription><rbf>Minimig</rbf><file path="game.vhd"/><reset/></mistergamedescription>"#,
        )
        .expect("write wrong core");
        assert!(validate_0mhz_mgl(&wrong_core)
            .expect_err("wrong core should fail")
            .contains("expected AO486"));

        let missing = dir.join("missing.mgl");
        std::fs::write(
            &missing,
            r#"<mistergamedescription><rbf>AO486</rbf><file path="missing.vhd"/><reset/></mistergamedescription>"#,
        )
        .expect("write missing payload");
        assert!(validate_0mhz_mgl(&missing)
            .expect_err("missing payload should fail")
            .contains("payload is missing"));

        std::fs::write(dir.join("game.vhd"), b"vhd").expect("write vhd");
        let no_reset = dir.join("no-reset.mgl");
        std::fs::write(
            &no_reset,
            r#"<mistergamedescription><rbf>AO486</rbf><file path="game.vhd"/></mistergamedescription>"#,
        )
        .expect("write no reset");
        assert!(validate_0mhz_mgl(&no_reset)
            .expect_err("missing reset should fail")
            .contains("no reset"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
