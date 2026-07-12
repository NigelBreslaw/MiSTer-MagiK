//! Shared metadata for collections that provide their own one-click launch artifacts.

use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::media_metadata::{inspect_mgl, resolve_mgl_payload_path, MglInspection};

pub const PREPARED_COLLECTION_ADAPTER_VERSION: u32 = 5;

pub fn storage_roots_for_library_roots(roots: &[String]) -> Vec<PathBuf> {
    let mut storage_roots = Vec::new();
    for root in roots {
        let storage_root = storage_root_for_library_root(Path::new(root));
        if !storage_roots.contains(&storage_root) {
            storage_roots.push(storage_root);
        }
    }
    storage_roots
}

fn storage_root_for_library_root(root: &Path) -> PathBuf {
    let mut prefix = PathBuf::new();
    for component in root.components() {
        let value = component.as_os_str().to_string_lossy();
        if matches!(
            value.as_ref(),
            "games" | "_Arcade" | "_Games" | "_DOS Games" | "_LLAPI" | "_Computer"
        ) {
            return prefix;
        }
        prefix.push(component.as_os_str());
    }
    root.to_path_buf()
}

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedLaunchDiagnostic {
    pub(crate) collection_id: PreparedCollectionId,
    pub(crate) status: &'static str,
    pub(crate) reason: String,
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
    validate_0mhz_mgl_inspection(path, &inspection)?;
    Ok(inspection)
}

pub(crate) fn validate_0mhz_mgl_inspection(
    path: &Path,
    inspection: &MglInspection,
) -> Result<(), String> {
    let rbf = inspection
        .rbf
        .as_deref()
        .ok_or_else(|| "0MHz MGL has no RBF".to_string())?;
    if !crate::library_db::normalize_id(rbf).ends_with("ao486") {
        return Err(format!("0MHz MGL targets {rbf}, expected AO486"));
    }
    if inspection.files.is_empty() {
        return Err("0MHz MGL has no file mount actions".to_string());
    }
    if inspection.reset_count == 0 {
        return Err("0MHz MGL has no reset action".to_string());
    }
    for action in &inspection.files {
        let payload = resolve_0mhz_payload_path(path, &action.path);
        if !payload.is_file() {
            return Err(format!(
                "0MHz MGL payload is missing: {}",
                payload.display()
            ));
        }
    }
    Ok(())
}

pub(crate) fn resolve_0mhz_payload_path(mgl_path: &Path, payload: &str) -> PathBuf {
    let local = resolve_mgl_payload_path(mgl_path, payload);
    if local.is_file() || payload.starts_with('/') || payload.starts_with("games/") {
        return local;
    }

    let Some(dos_games_root) = mgl_path.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("_DOS Games"))
    }) else {
        return local;
    };
    let Some(storage_root) = dos_games_root.parent() else {
        return local;
    };
    let collection_payload = storage_root.join("games/AO486").join(payload);
    if collection_payload.is_file() {
        collection_payload
    } else {
        local
    }
}

pub(crate) fn validate_neon68k_mgl(path: &Path) -> Result<MglInspection, String> {
    let inspection = inspect_mgl(path)?;
    validate_neon68k_mgl_inspection(path, &inspection)?;
    Ok(inspection)
}

pub(crate) fn validate_neon68k_mgl_inspection(
    path: &Path,
    inspection: &MglInspection,
) -> Result<(), String> {
    let rbf = inspection
        .rbf
        .as_deref()
        .ok_or_else(|| "Neon68K MGL has no RBF".to_string())?;
    if !crate::library_db::normalize_id(rbf).ends_with("x68000") {
        return Err(format!("Neon68K MGL targets {rbf}, expected X68000"));
    }
    if inspection
        .setname
        .as_deref()
        .is_none_or(|setname| setname.trim().is_empty())
    {
        return Err("Neon68K MGL has no setname".to_string());
    }
    let mut hdf_count = 0usize;
    for action in &inspection.files {
        let payload = resolve_mgl_payload_path(path, &action.path);
        if !payload.is_file() {
            return Err(format!(
                "Neon68K MGL payload is missing: {}",
                payload.display()
            ));
        }
        if payload
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("hdf"))
        {
            hdf_count = hdf_count.saturating_add(1);
        }
    }
    if hdf_count == 0 {
        return Err("Neon68K MGL has no HDF mount action".to_string());
    }
    Ok(())
}

pub(crate) fn neon68k_source_category(path: &Path) -> Option<String> {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .find_map(|component| {
            let normalized = component.to_ascii_lowercase();
            if normalized.contains("keyboard") || normalized.contains("mouse") {
                Some("Keyboard + Mouse".to_string())
            } else if normalized.contains("major") && normalized.contains("bug") {
                Some("Major Bugs".to_string())
            } else if normalized.contains("minor") && normalized.contains("bug") {
                Some("Minor Bugs".to_string())
            } else {
                None
            }
        })
}

pub(crate) fn oneload64_provenance(path: &Path) -> Option<PreparedLaunchProvenance> {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("crt"))
    {
        return None;
    }
    let install_root = path.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_oneload64_install_name)
    })?;
    if !oneload64_root_has_signature(install_root) || oneload64_path_is_excluded(path, install_root)
    {
        return None;
    }
    Some(PreparedLaunchProvenance::prepared(
        PreparedCollectionId::OneLoad64,
    ))
}

fn oneload64_root_has_signature(root: &Path) -> bool {
    // A catalog build runs in a fresh standalone process. The install root
    // cannot meaningfully change underneath that one scan, so key this
    // process-local fact by path instead of statting the same exFAT directory
    // once for every CRT payload.
    type SignatureCache = std::collections::HashMap<std::path::PathBuf, bool>;
    static CACHE: OnceLock<Mutex<SignatureCache>> = OnceLock::new();
    let key = root.to_path_buf();
    let cache = CACHE.get_or_init(|| Mutex::new(SignatureCache::new()));
    if let Some(cached) = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .copied()
    {
        return cached;
    }
    let valid = std::fs::read_dir(root).ok().is_some_and(|entries| {
        entries.flatten().any(|entry| {
            entry.file_type().ok().is_some_and(|kind| kind.is_dir())
                && matches!(
                    compact_name(&entry.file_name().to_string_lossy()).as_str(),
                    "multiload64" | "dumps" | "alternativeformats"
                )
        })
    });
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, valid);
    valid
}

fn oneload64_path_is_excluded(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).ok().is_some_and(|relative| {
        relative.components().any(|component| {
            matches!(
                compact_name(&component.as_os_str().to_string_lossy()).as_str(),
                "dumps" | "alternativeformats" | "extras" | "docs" | "documentation"
            )
        })
    })
}

fn compact_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_oneload64_install_name(value: &str) -> bool {
    compact_name(value).starts_with("oneload64")
}

pub fn validate_prepared_launch_path(path: &Path) -> Result<bool, String> {
    let is_mgl = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mgl"));
    if is_mgl && path_has_component(path, "_DOS Games") {
        validate_0mhz_mgl(path)?;
        return Ok(true);
    }
    if is_mgl && path_has_component(path, "X68000 Games") {
        validate_neon68k_mgl(path)?;
        return Ok(true);
    }
    let oneload64_install = path.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_oneload64_install_name)
    });
    if let Some(install_root) = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("crt"))
        .then_some(oneload64_install)
        .flatten()
    {
        if !path.is_file() {
            return Err(format!(
                "prepared C64 payload is missing: {}",
                path.display()
            ));
        }
        if !oneload64_root_has_signature(install_root) {
            return Err(format!(
                "OneLoad64 installation signature is missing: {}",
                install_root.display()
            ));
        }
        if oneload64_path_is_excluded(path, install_root) {
            return Err(format!(
                "prepared C64 payload is outside the primary OneLoad64 trees: {}",
                path.display()
            ));
        }
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn diagnostic_for_candidate(
    path: &Path,
    platform_id: &str,
) -> Option<PreparedLaunchDiagnostic> {
    let is_mgl = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mgl"));
    if is_mgl && platform_id == "dos" && path_has_component(path, "_DOS Games") {
        return validate_0mhz_mgl(path)
            .err()
            .map(|reason| PreparedLaunchDiagnostic {
                collection_id: PreparedCollectionId::ZeroMhz,
                status: "invalid",
                reason,
            });
    }
    if is_mgl && path_has_component(path, "X68000 Games") {
        return validate_neon68k_mgl(path)
            .err()
            .map(|reason| PreparedLaunchDiagnostic {
                collection_id: PreparedCollectionId::Neon68k,
                status: "invalid",
                reason,
            });
    }
    let install_root = path.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_oneload64_install_name)
    })?;
    if oneload64_path_is_excluded(path, install_root) {
        return Some(PreparedLaunchDiagnostic {
            collection_id: PreparedCollectionId::OneLoad64,
            status: "excluded",
            reason: "non-primary OneLoad64 tree".to_string(),
        });
    }
    (!oneload64_root_has_signature(install_root)).then(|| PreparedLaunchDiagnostic {
        collection_id: PreparedCollectionId::OneLoad64,
        status: "invalid",
        reason: "OneLoad64 directory is missing its collection signature".to_string(),
    })
}

fn path_has_component(path: &Path, expected: &str) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|component| component.eq_ignore_ascii_case(expected))
    })
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
                <rbf>_Computer/AO486</rbf>
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
    fn zero_mhz_validation_resolves_split_launcher_and_payload_roots() {
        let storage = fixture_dir("0mhz-split-root");
        let launchers = storage.join("_DOS Games");
        let payload = storage.join("games/AO486/media/doom/doom.vhd");
        std::fs::create_dir_all(&launchers).expect("create launcher root");
        std::fs::create_dir_all(payload.parent().expect("payload parent"))
            .expect("create payload root");
        std::fs::write(&payload, b"vhd").expect("write payload");
        let mgl = launchers.join("Doom.mgl");
        std::fs::write(
            &mgl,
            r#"<mistergamedescription>
                <rbf>_computer/ao486</rbf>
                <file type="s" index="2" path="media/doom/doom.vhd"/>
                <reset delay="1"/>
            </mistergamedescription>"#,
        )
        .expect("write mgl");

        let inspection = validate_0mhz_mgl(&mgl).expect("validate split 0MHz layout");

        assert_eq!(inspection.files.len(), 1);
        assert_eq!(
            resolve_0mhz_payload_path(&mgl, &inspection.files[0].path),
            payload
        );
        let _ = std::fs::remove_dir_all(storage);
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

    #[test]
    fn neon68k_validation_requires_x68000_setname_and_hdf() {
        let dir = fixture_dir("neon68k-valid");
        let issue_dir = dir.join("Keyboard + Mouse");
        std::fs::create_dir_all(&issue_dir).expect("create issue dir");
        std::fs::write(issue_dir.join("game.hdf"), b"hdf").expect("write hdf");
        let mgl = issue_dir.join("Akumajou Dracula.mgl");
        std::fs::write(
            &mgl,
            r#"<mistergamedescription><rbf>_Computer/X68000</rbf><setname>Akumajou</setname><file index="0" path="game.hdf"/></mistergamedescription>"#,
        )
        .expect("write MGL");

        let inspection = validate_neon68k_mgl(&mgl).expect("validate Neon68K MGL");

        assert_eq!(inspection.setname.as_deref(), Some("Akumajou"));
        assert_eq!(
            neon68k_source_category(&mgl).as_deref(),
            Some("Keyboard + Mouse")
        );

        let missing_setname = dir.join("missing-setname.mgl");
        std::fs::write(
            &missing_setname,
            r#"<mistergamedescription><rbf>X68000</rbf><file path="Keyboard + Mouse/game.hdf"/></mistergamedescription>"#,
        )
        .expect("write missing setname MGL");
        assert!(validate_neon68k_mgl(&missing_setname)
            .expect_err("missing setname should fail")
            .contains("no setname"));

        let missing_hdf = dir.join("missing-hdf.mgl");
        std::fs::write(
            &missing_hdf,
            r#"<mistergamedescription><rbf>X68000</rbf><setname>Missing</setname><file path="missing.hdf"/></mistergamedescription>"#,
        )
        .expect("write missing HDF MGL");
        assert!(validate_neon68k_mgl(&missing_hdf)
            .expect_err("missing HDF should fail")
            .contains("payload is missing"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn oneload64_requires_install_signature_and_excludes_non_primary_trees() {
        let dir = fixture_dir("oneload64");
        let install = dir.join("OneLoad64 Games Collection v4");
        let multi = install.join("MultiLoad64");
        let dumps = install.join("Dumps");
        let alternatives = install.join("AlternativeFormats");
        let extras = install.join("Extras");
        for path in [&multi, &dumps, &alternatives, &extras] {
            std::fs::create_dir_all(path).expect("create collection dir");
        }
        let primary = install.join("Impossible Mission.crt");
        let multiload = multi.join("Summer Games.crt");
        let dump = dumps.join("Dump.crt");
        let alternative = alternatives.join("Alternative.crt");
        let extra = extras.join("Extra.crt");
        for path in [&primary, &multiload, &dump, &alternative, &extra] {
            std::fs::write(path, b"crt").expect("write CRT");
        }

        assert!(oneload64_provenance(&primary).is_some());
        assert!(oneload64_provenance(&multiload).is_some());
        assert!(oneload64_provenance(&dump).is_none());
        assert!(oneload64_provenance(&alternative).is_none());
        assert!(oneload64_provenance(&extra).is_none());
        assert_eq!(validate_prepared_launch_path(&primary), Ok(true));
        assert!(validate_prepared_launch_path(&dump)
            .expect_err("excluded prepared path should fail")
            .contains("outside the primary"));

        let unmarked = dir.join("General C64 CRTs/Game.crt");
        std::fs::create_dir_all(unmarked.parent().expect("unmarked parent"))
            .expect("create unmarked dir");
        std::fs::write(&unmarked, b"crt").expect("write unmarked CRT");
        assert!(oneload64_provenance(&unmarked).is_none());
        assert_eq!(validate_prepared_launch_path(&unmarked), Ok(false));
        let _ = std::fs::remove_dir_all(dir);
    }
}
