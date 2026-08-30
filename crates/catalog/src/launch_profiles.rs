// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Source-derived launch profiles for MiSTer library scanning.
//!
//! Profiles are built from explicit special cases plus a generated manifest of
//! generic Main file-picker cores. Runtime scans activate manifest rows only
//! when the matching core is installed, so game folders alone do not become
//! launchable by guesswork.

use crate::catalog_discovery;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;

pub const PROFILE_SET_VERSION: u32 = 9;
pub const CORE_LAUNCH_MANIFEST_VERSION: u32 = 2;

const CORE_LAUNCH_MANIFEST_JSON: &str = include_str!("../data/core_launch_manifest.json");

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RuleSourceKind {
    MainSource,
    Mgl,
    Mra,
    ConfStr,
    MagikProfile,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuleProvenance {
    pub kind: RuleSourceKind,
    pub detail: String,
}

impl RuleProvenance {
    pub fn main(detail: impl Into<String>) -> Self {
        Self {
            kind: RuleSourceKind::MainSource,
            detail: detail.into(),
        }
    }

    pub fn mgl(detail: impl Into<String>) -> Self {
        Self {
            kind: RuleSourceKind::Mgl,
            detail: detail.into(),
        }
    }

    pub fn mra(detail: impl Into<String>) -> Self {
        Self {
            kind: RuleSourceKind::Mra,
            detail: detail.into(),
        }
    }

    pub fn conf_str(detail: impl Into<String>) -> Self {
        Self {
            kind: RuleSourceKind::ConfStr,
            detail: detail.into(),
        }
    }

    pub fn magik(detail: impl Into<String>) -> Self {
        Self {
            kind: RuleSourceKind::MagikProfile,
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MountKind {
    Launcher,
    LoadFile,
    MountImage,
    Core,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MountSpec {
    pub kind: MountKind,
    pub index: u8,
    pub delay_secs: u8,
}

impl MountSpec {
    pub const fn launcher() -> Self {
        Self {
            kind: MountKind::Launcher,
            index: 0,
            delay_secs: 0,
        }
    }

    pub const fn load_file(index: u8) -> Self {
        Self {
            kind: MountKind::LoadFile,
            index,
            delay_secs: 1,
        }
    }

    pub const fn mount_image(index: u8) -> Self {
        Self {
            kind: MountKind::MountImage,
            index,
            delay_secs: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PayloadDisposition {
    Playable,
    AttachedMedia,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionListing {
    pub entry_path: String,
    pub genre: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionRule {
    pub archive_extensions: Vec<String>,
    pub file_name_contains: Vec<String>,
    pub listings: Vec<CollectionListing>,
    pub provenance: RuleProvenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IgnoreReason {
    Bios,
    CueTrack,
    CoreBinary,
    SaveMedia,
    SupportArchive,
    Firmware,
    StateOrConfiguration,
    Tool,
    InstallerOrBlankMedia,
    Demo,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PayloadRule {
    pub extensions: Vec<String>,
    pub mount: MountSpec,
    pub disposition: PayloadDisposition,
    pub provenance: RuleProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IgnoreRule {
    pub file_names: Vec<String>,
    pub extensions: Vec<String>,
    pub reason: IgnoreReason,
    pub provenance: RuleProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchProfile {
    pub id: String,
    pub system_id: String,
    pub category: String,
    pub title: String,
    pub core_name: String,
    pub core_path: Option<String>,
    pub game_dirs: Vec<String>,
    pub payload_rules: Vec<PayloadRule>,
    pub archive_entry_rules: Vec<PayloadRule>,
    pub collection_rules: Vec<CollectionRule>,
    pub ignore_rules: Vec<IgnoreRule>,
    pub provenance: RuleProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileSet {
    profiles: Vec<LaunchProfile>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeProfilePlan {
    pub(crate) game_dir_name: String,
    pub(crate) decision: RuntimeProfileDecision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeProfileDecision {
    Catalogable { profile: Box<LaunchProfile> },
    NoInstalledCore,
    EmptyOrMediaOnly,
    NoKnownPayloadExtension,
    Ambiguous { core_ids: Vec<String> },
}

impl ProfileSet {
    pub fn all() -> Self {
        Self {
            profiles: all_profiles(),
        }
    }

    pub fn for_roots(roots: &[String]) -> Self {
        Self {
            profiles: active_profiles_for_roots(roots),
        }
    }

    #[cfg(feature = "builder")]
    pub(crate) fn try_for_roots(roots: &[String]) -> Result<Self, String> {
        Ok(Self {
            profiles: try_active_profiles_for_roots(roots)?,
        })
    }

    pub fn profiles(&self) -> &[LaunchProfile] {
        &self.profiles
    }

    pub fn into_profiles(self) -> Vec<LaunchProfile> {
        self.profiles
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfilePathClass {
    Payload {
        rule: PayloadRule,
    },
    Collection {
        rule: CollectionRule,
    },
    Ignored {
        reason: IgnoreReason,
        provenance: RuleProvenance,
    },
    NotMatched,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(feature = "builder")]
pub(crate) enum BorrowedProfilePathClass<'a> {
    Payload { rule: &'a PayloadRule },
    Collection { rule: &'a CollectionRule },
    Ignored { reason: IgnoreReason },
    NotMatched,
}

impl LaunchProfile {
    #[cfg(feature = "builder")]
    pub(crate) fn classify_path_borrowed(&self, path: &Path) -> BorrowedProfilePathClass<'_> {
        for rule in &self.ignore_rules {
            if rule.matches(path) {
                return BorrowedProfilePathClass::Ignored {
                    reason: rule.reason,
                };
            }
        }

        if let Some(reason) = generic_support_reason(path) {
            return BorrowedProfilePathClass::Ignored { reason };
        }

        for rule in &self.collection_rules {
            if rule.matches(path) {
                return BorrowedProfilePathClass::Collection { rule };
            }
        }

        let ext = path_ext(path);
        for rule in &self.payload_rules {
            if ext
                .as_deref()
                .is_some_and(|ext| contains_ignore_ascii_case(&rule.extensions, ext))
            {
                return BorrowedProfilePathClass::Payload { rule };
            }
        }

        BorrowedProfilePathClass::NotMatched
    }

    pub fn classify_path(&self, path: &Path) -> ProfilePathClass {
        for rule in &self.ignore_rules {
            if rule.matches(path) {
                return ProfilePathClass::Ignored {
                    reason: rule.reason,
                    provenance: rule.provenance.clone(),
                };
            }
        }

        if let Some(reason) = generic_support_reason(path) {
            return ProfilePathClass::Ignored {
                reason,
                provenance: RuleProvenance::magik(
                    "Generic catalog content-role classifier excluded support material",
                ),
            };
        }

        for rule in &self.collection_rules {
            if rule.matches(path) {
                return ProfilePathClass::Collection { rule: rule.clone() };
            }
        }

        let ext = path_ext(path);
        for rule in &self.payload_rules {
            if ext
                .as_deref()
                .is_some_and(|ext| contains_ignore_ascii_case(&rule.extensions, ext))
            {
                return ProfilePathClass::Payload { rule: rule.clone() };
            }
        }

        ProfilePathClass::NotMatched
    }

    pub fn classify_archive_entry(&self, path: &Path) -> Option<PayloadRule> {
        if generic_support_reason(path).is_some() {
            return None;
        }
        let ext = path_ext(path)?;
        self.archive_entry_rules
            .iter()
            .find(|rule| contains_ignore_ascii_case(&rule.extensions, &ext))
            .cloned()
    }
}

fn generic_support_reason(path: &Path) -> Option<IgnoreReason> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let stem = path.file_stem()?.to_string_lossy().to_ascii_lowercase();
    let ext = path_ext(path).unwrap_or_default();
    let support_directory = path.components().any(|component| {
        component.as_os_str().to_str().is_some_and(|part| {
            matches!(
                part.to_ascii_lowercase().as_str(),
                "demo" | "demos" | "palette" | "palettes"
            )
        })
    });
    let numbered_boot_rom = ext == "rom"
        && stem
            .strip_prefix("boot")
            .is_some_and(|tail| tail.is_empty() || tail.chars().all(|ch| ch.is_ascii_digit()));
    let versioned_firmware_rom = ext == "rom"
        && (stem.contains("kickstart")
            || stem
                .split('.')
                .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit())));
    if numbered_boot_rom
        || versioned_firmware_rom
        || name.contains("bios")
        || matches!(name.as_str(), "cd_bios.rom" | "riscos.rom")
    {
        return Some(IgnoreReason::Firmware);
    }
    if matches!(
        ext.as_str(),
        "act" | "gbp" | "nvr" | "jce" | "jmc" | "srm" | "sav"
    ) || stem.contains("eeprom")
        || stem.contains("memorytrack")
    {
        return Some(IgnoreReason::StateOrConfiguration);
    }
    if matches!(ext.as_str(), "sh" | "cmd" | "bat") || stem.contains("bin2dsk") {
        return Some(IgnoreReason::Tool);
    }
    if stem.contains("blank")
        || stem.contains("empty")
        || stem.contains("disk605")
        || matches!(stem.as_str(), "sdcard" | "alt_roms")
    {
        return Some(IgnoreReason::InstallerOrBlankMedia);
    }
    if support_directory
        || (stem.contains("demo") && ext != "txt")
        || matches!(name.as_str(), "env.cas" | "galaxy.cas" | "spores.cas")
    {
        return Some(IgnoreReason::Demo);
    }
    None
}

impl IgnoreRule {
    fn matches(&self, path: &Path) -> bool {
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if contains_ignore_ascii_case(&self.file_names, file_name) {
            return true;
        }

        path_ext(path)
            .as_deref()
            .is_some_and(|ext| contains_ignore_ascii_case(&self.extensions, ext))
    }
}

impl CollectionRule {
    fn matches(&self, path: &Path) -> bool {
        let Some(ext) = path_ext(path) else {
            return false;
        };
        if !contains_ignore_ascii_case(&self.archive_extensions, &ext) {
            return false;
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        self.file_name_contains
            .iter()
            .all(|needle| name.contains(&needle.to_ascii_lowercase()))
    }
}

#[derive(Debug, Deserialize)]
struct CoreLaunchManifest {
    schema: u32,
    profiles: Vec<CoreLaunchManifestRow>,
}

#[derive(Clone, Debug, Deserialize)]
struct CoreLaunchManifestRow {
    id: String,
    system_id: String,
    category: String,
    title: String,
    core_name: String,
    core_path: String,
    #[serde(default)]
    compatible_core_names: Vec<String>,
    game_dirs: Vec<String>,
    extensions: Vec<String>,
    archive_entries: bool,
    evidence: String,
}

pub fn builtin_profiles() -> Vec<LaunchProfile> {
    all_profiles()
}

/// Filesystem facts and profile state shared by one cold catalog scan.
///
/// Building the plan reads the installed cores and enumerates only the
/// unclaimed top-level game directory headers.  Callers that already collect
/// payload facts while walking those directories can then finalize the active
/// profile set without repeating either discovery step.
#[derive(Clone, Debug)]
pub(crate) struct CatalogScanPlan {
    installed_cores: Vec<catalog_discovery::InstalledCore>,
    all_game_dir_headers: Vec<catalog_discovery::GameDirHeader>,
    game_dir_headers: Vec<catalog_discovery::GameDirHeader>,
    base_profiles: Vec<LaunchProfile>,
}

impl CatalogScanPlan {
    pub(crate) fn for_roots(roots: &[String]) -> Self {
        crate::cooperative_work::checkpoint();
        let core_started = Instant::now();
        let installed_cores = catalog_discovery::installed_cores_for_roots(roots);
        let core_us = core_started.elapsed().as_micros() as u64;
        crate::cooperative_work::checkpoint();
        let game_headers_started = Instant::now();
        let all_game_dir_headers =
            catalog_discovery::top_level_game_dir_headers_for_roots_excluding(
                roots,
                &BTreeSet::new(),
            );
        let game_headers_us = game_headers_started.elapsed().as_micros() as u64;
        crate::library_db::report_library_scan_timing(
            "scan_plan_cores",
            core_us,
            format!("cores={}", installed_cores.len()),
        );
        crate::library_db::report_library_scan_timing(
            "scan_plan_game_headers",
            game_headers_us,
            format!("headers={}", all_game_dir_headers.len()),
        );
        let profiles_started = Instant::now();
        let base_profiles = base_profiles_for_installed_cores(&installed_cores);
        let active_game_dirs = active_profile_game_dirs(&base_profiles);
        let game_dir_headers = all_game_dir_headers
            .iter()
            .filter(|header| !active_game_dirs.contains(&header.name.to_ascii_lowercase()))
            .cloned()
            .collect::<Vec<_>>();
        crate::library_db::report_library_scan_timing(
            "scan_plan_profiles",
            profiles_started.elapsed().as_micros() as u64,
            format!(
                "base_profiles={} runtime_headers={}",
                base_profiles.len(),
                game_dir_headers.len()
            ),
        );
        Self {
            installed_cores,
            all_game_dir_headers,
            game_dir_headers,
            base_profiles,
        }
    }

    #[cfg(feature = "builder")]
    pub(crate) fn try_for_roots(roots: &[String]) -> Result<Self, String> {
        crate::cooperative_work::checkpoint();
        let core_started = Instant::now();
        let installed_cores = catalog_discovery::installed_cores_for_roots_checked(roots)?;
        let core_us = core_started.elapsed().as_micros() as u64;
        crate::cooperative_work::checkpoint();
        let game_headers_started = Instant::now();
        let all_game_dir_headers =
            catalog_discovery::top_level_game_dir_headers_for_roots_excluding_checked(
                roots,
                &BTreeSet::new(),
            )?;
        let game_headers_us = game_headers_started.elapsed().as_micros() as u64;
        crate::library_db::report_library_scan_timing(
            "scan_plan_cores",
            core_us,
            format!("cores={}", installed_cores.len()),
        );
        crate::library_db::report_library_scan_timing(
            "scan_plan_game_headers",
            game_headers_us,
            format!("headers={}", all_game_dir_headers.len()),
        );
        let profiles_started = Instant::now();
        let base_profiles = base_profiles_for_installed_cores(&installed_cores);
        let active_game_dirs = active_profile_game_dirs(&base_profiles);
        let game_dir_headers = all_game_dir_headers
            .iter()
            .filter(|header| !active_game_dirs.contains(&header.name.to_ascii_lowercase()))
            .cloned()
            .collect::<Vec<_>>();
        crate::library_db::report_library_scan_timing(
            "scan_plan_profiles",
            profiles_started.elapsed().as_micros() as u64,
            format!(
                "base_profiles={} runtime_headers={}",
                base_profiles.len(),
                game_dir_headers.len()
            ),
        );
        Ok(Self {
            installed_cores,
            all_game_dir_headers,
            game_dir_headers,
            base_profiles,
        })
    }

    pub(crate) fn installed_cores(&self) -> &[catalog_discovery::InstalledCore] {
        &self.installed_cores
    }

    pub(crate) fn game_dir_headers(&self) -> &[catalog_discovery::GameDirHeader] {
        &self.game_dir_headers
    }

    pub(crate) fn all_game_dir_headers(&self) -> &[catalog_discovery::GameDirHeader] {
        &self.all_game_dir_headers
    }

    /// Return a header from the checked `/games` enumeration only when its
    /// exact requested path was proved to be an ordinary directory. A case
    /// variant or uncertain entry deliberately falls through to the old
    /// path-based check in the generic scanner.
    pub(crate) fn header_for_known_game_dir(
        &self,
        storage_root: &Path,
        game_dir: &str,
    ) -> Option<&catalog_discovery::GameDirHeader> {
        let expected = storage_root.join("games").join(game_dir);
        self.all_game_dir_headers
            .iter()
            .find(|header| header.confirmed_directory && header.path == expected)
    }

    pub(crate) fn base_profiles(&self) -> &[LaunchProfile] {
        &self.base_profiles
    }

    pub(crate) fn finalize_profiles(
        &self,
        game_dirs: &[catalog_discovery::GameDirFact],
    ) -> Vec<LaunchProfile> {
        finalize_profiles_from_facts(&self.base_profiles, &self.installed_cores, game_dirs)
    }

    /// Resolves the profile for one walker-derived game-directory fact.
    ///
    /// This deliberately delegates to finalization rather than using the
    /// tentative streaming profile set, so an empty or unsupported runtime
    /// directory never becomes a launch target.
    pub(crate) fn profile_for_game_dir_facts(
        &self,
        game_dir: &catalog_discovery::GameDirFact,
    ) -> Option<LaunchProfile> {
        self.finalize_profiles(std::slice::from_ref(game_dir))
            .into_iter()
            .find(|profile| {
                profile
                    .game_dirs
                    .iter()
                    .any(|dir| dir.eq_ignore_ascii_case(&game_dir.name))
            })
    }
}

pub fn active_profiles_for_roots(roots: &[String]) -> Vec<LaunchProfile> {
    let plan = CatalogScanPlan::for_roots(roots);
    let game_dirs = plan
        .game_dir_headers()
        .iter()
        .cloned()
        .map(catalog_discovery::game_dir_payload_facts_for_header)
        .collect::<Vec<_>>();
    plan.finalize_profiles(&game_dirs)
}

#[cfg(feature = "builder")]
pub(crate) fn try_active_profiles_for_roots(
    roots: &[String],
) -> Result<Vec<LaunchProfile>, String> {
    let plan = CatalogScanPlan::try_for_roots(roots)?;
    let game_dirs = plan
        .game_dir_headers()
        .iter()
        .cloned()
        .map(catalog_discovery::game_dir_payload_facts_for_header_checked)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(plan.finalize_profiles(&game_dirs))
}

pub(crate) fn active_profiles_for_roots_with_facts(
    installed_cores: &[catalog_discovery::InstalledCore],
    game_dirs: &[catalog_discovery::GameDirFact],
) -> Vec<LaunchProfile> {
    let base_profiles = base_profiles_for_installed_cores(installed_cores);
    finalize_profiles_from_facts(&base_profiles, installed_cores, game_dirs)
}

fn base_profiles_for_installed_cores(
    installed_cores: &[catalog_discovery::InstalledCore],
) -> Vec<LaunchProfile> {
    let installed = installed_cores
        .iter()
        .map(|core| core.core_id.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let compatible_physical_cores = installed_cores.iter().fold(
        BTreeMap::<String, &catalog_discovery::InstalledCore>::new(),
        |mut cores, core| {
            if installed_core_is_manifest_compatible(core) {
                let key = catalog_discovery::compact_system_name(&core.core_id);
                cores
                    .entry(key)
                    .and_modify(|existing| {
                        if installed_core_is_exact_physical(core)
                            && !installed_core_is_exact_physical(existing)
                        {
                            *existing = core;
                        }
                    })
                    .or_insert(core);
            }
            cores
        },
    );
    let descriptor_dirs = installed_cores
        .iter()
        .filter(|core| !installed_core_is_manifest_compatible(core))
        .filter(|core| {
            !compatible_physical_cores
                .contains_key(&catalog_discovery::compact_system_name(&core.core_id))
        })
        .map(|core| catalog_discovery::compact_system_name(&core.core_id))
        .collect::<BTreeSet<_>>();
    let mut profiles = special_profiles();
    profiles.extend(
        generic_manifest_profiles()
            .into_iter()
            .filter_map(|mut profile| {
                let profile_key = catalog_discovery::compact_system_name(&profile.core_name);
                let compatible_core = compatible_physical_cores.get(&profile_key).copied();
                if compatible_core.is_none()
                    && !installed
                        .contains(&canonical_core_id(&profile.core_name).to_ascii_lowercase())
                {
                    return None;
                }
                if compatible_core.is_none() && descriptor_dirs.contains(&profile_key) {
                    return None;
                }
                if let Some(core) = compatible_core {
                    // Canonical manifest profiles deliberately retain their logical core
                    // selector (for example `_Console/SNES`). Main resolves that selector
                    // to the newest installed dated RBF at handoff. Persisting the exact
                    // file observed during a catalog build would pin every game to an old
                    // core after Update All installs a replacement.
                    //
                    // A descriptor-backed compatible system is different: its logical
                    // identity can intentionally target another physical core, so retain
                    // that target while still allowing the launcher to remove its date.
                    if !installed_core_is_exact_physical(core) {
                        profile.core_path = relative_core_path_for_installed_core(&core.path);
                    }
                }
                profile.game_dirs.retain(|dir| {
                    !descriptor_dirs.contains(&catalog_discovery::compact_system_name(dir))
                });
                (!profile.game_dirs.is_empty()).then_some(profile)
            }),
    );
    profiles
}

fn installed_core_is_exact_physical(core: &catalog_discovery::InstalledCore) -> bool {
    core.path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| {
            catalog_discovery::compact_system_name(&canonical_core_id(stem))
                == catalog_discovery::compact_system_name(&core.core_id)
        })
}

fn installed_core_is_manifest_compatible(core: &catalog_discovery::InstalledCore) -> bool {
    installed_core_is_exact_physical(core)
        || (generic_manifest_profile_for_core(&core.core_id).is_some()
            && core_path_is_compatible_with_canonical_system(&core.core_id, &core.path))
}

fn finalize_profiles_from_facts(
    base_profiles: &[LaunchProfile],
    installed_cores: &[catalog_discovery::InstalledCore],
    game_dirs: &[catalog_discovery::GameDirFact],
) -> Vec<LaunchProfile> {
    let mut profiles = base_profiles.to_vec();
    let mut active_game_dirs = active_profile_game_dirs(&profiles);
    for plan in runtime_profile_plans_for_game_dirs_with_cores(
        game_dirs,
        installed_cores,
        &active_game_dirs,
    ) {
        let RuntimeProfileDecision::Catalogable { profile } = plan.decision else {
            continue;
        };
        activate_runtime_profile(*profile, &mut profiles, &mut active_game_dirs);
    }
    profiles
}

fn activate_runtime_profile(
    mut profile: LaunchProfile,
    profiles: &mut Vec<LaunchProfile>,
    active_game_dirs: &mut BTreeSet<String>,
) {
    profile
        .game_dirs
        .retain(|dir| active_game_dirs.insert(dir.to_ascii_lowercase()));
    if profile.game_dirs.is_empty() {
        return;
    }
    if let Some(existing) = profiles
        .iter_mut()
        .find(|existing| existing.id == profile.id)
    {
        for dir in profile.game_dirs {
            if !existing
                .game_dirs
                .iter()
                .any(|existing_dir| existing_dir.eq_ignore_ascii_case(&dir))
            {
                existing.game_dirs.push(dir);
            }
        }
    } else {
        profiles.push(profile);
    }
}

pub fn generic_manifest_profile_for_game_dir(game_dir: &str) -> Option<LaunchProfile> {
    generic_manifest_profiles_cached()
        .iter()
        .find(|profile| {
            profile
                .game_dirs
                .iter()
                .any(|dir| dir.eq_ignore_ascii_case(game_dir))
        })
        .cloned()
}

pub fn generic_manifest_profile_for_core(core_id: &str) -> Option<LaunchProfile> {
    let normalized = canonical_core_id(core_id);
    generic_manifest_profiles_cached()
        .iter()
        .find(|profile| profile.core_name.eq_ignore_ascii_case(&normalized))
        .cloned()
}

pub(crate) fn profile_for_launch_target_id<'a>(
    profiles: &'a [LaunchProfile],
    profile_id: &str,
) -> Option<&'a LaunchProfile> {
    profiles
        .iter()
        .find(|profile| profile.id.as_str() == profile_id)
        .or_else(|| {
            let mut matches = profiles
                .iter()
                .filter(|profile| profile.system_id.as_str() == profile_id);
            let profile = matches.next()?;
            matches.next().is_none().then_some(profile)
        })
}

fn active_profile_game_dirs(profiles: &[LaunchProfile]) -> BTreeSet<String> {
    profiles
        .iter()
        .flat_map(|profile| profile.game_dirs.iter())
        .map(|dir| dir.to_ascii_lowercase())
        .collect()
}

pub fn profile_for_game_dir<'a>(
    profiles: &'a [LaunchProfile],
    game_dir: &str,
) -> Option<&'a LaunchProfile> {
    profiles.iter().find(|profile| {
        profile
            .game_dirs
            .iter()
            .any(|dir| dir.eq_ignore_ascii_case(game_dir))
    })
}

pub fn installed_core_ids_for_roots(roots: &[String]) -> BTreeSet<String> {
    catalog_discovery::installed_cores_for_roots(roots)
        .into_iter()
        .map(|core| core.core_id.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
fn runtime_profile_plans_for_roots_with_cores(
    roots: &[String],
    cores: &[catalog_discovery::InstalledCore],
    active_game_dirs: &BTreeSet<String>,
) -> Vec<RuntimeProfilePlan> {
    catalog_discovery::top_level_game_dir_headers_for_roots_excluding(roots, active_game_dirs)
        .into_iter()
        .map(|game_dir| runtime_profile_plan_for_game_dir_header(game_dir, cores))
        .collect()
}

#[cfg(test)]
fn runtime_profile_plans_for_roots(roots: &[String]) -> Vec<RuntimeProfilePlan> {
    let cores = catalog_discovery::installed_cores_for_roots(roots);
    runtime_profile_plans_for_roots_with_cores(roots, &cores, &BTreeSet::new())
}

pub(crate) fn runtime_profile_plans_for_game_dirs_with_cores(
    game_dirs: &[catalog_discovery::GameDirFact],
    cores: &[catalog_discovery::InstalledCore],
    active_game_dirs: &BTreeSet<String>,
) -> Vec<RuntimeProfilePlan> {
    game_dirs
        .iter()
        .filter(|game_dir| !active_game_dirs.contains(&game_dir.name.to_ascii_lowercase()))
        .cloned()
        .map(|game_dir| runtime_profile_plan_for_game_dir(game_dir, cores))
        .collect()
}

#[cfg(test)]
fn runtime_profile_plan_for_game_dir_header(
    game_dir: catalog_discovery::GameDirHeader,
    cores: &[catalog_discovery::InstalledCore],
) -> RuntimeProfilePlan {
    let exact_or_alias_candidates = runtime_core_candidates_by_dir_name(&game_dir.name, cores);
    let decision = match exact_or_alias_candidates.as_slice() {
        [] => {
            runtime_profile_plan_for_game_dir(
                catalog_discovery::game_dir_payload_facts_for_header(game_dir.clone()),
                cores,
            )
            .decision
        }
        [candidate] => runtime_profile_decision_for_named_candidate(&game_dir, candidate),
        _ => {
            let facts = catalog_discovery::game_dir_payload_facts_for_header(game_dir.clone());
            if facts.has_payloadish_files() {
                RuntimeProfileDecision::Ambiguous {
                    core_ids: exact_or_alias_candidates
                        .iter()
                        .map(|candidate| candidate.core.core_id.clone())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                }
            } else {
                RuntimeProfileDecision::EmptyOrMediaOnly
            }
        }
    };
    RuntimeProfilePlan {
        game_dir_name: game_dir.name,
        decision,
    }
}

fn runtime_profile_plan_for_game_dir(
    game_dir: catalog_discovery::GameDirFact,
    cores: &[catalog_discovery::InstalledCore],
) -> RuntimeProfilePlan {
    let decision = if !game_dir.has_payloadish_files() {
        RuntimeProfileDecision::EmptyOrMediaOnly
    } else {
        let candidates = runtime_core_candidates(&game_dir, cores);
        match candidates.as_slice() {
            [] => RuntimeProfileDecision::NoInstalledCore,
            [candidate] => runtime_profile_for_match(&game_dir, candidate)
                .map(|profile| RuntimeProfileDecision::Catalogable {
                    profile: Box::new(profile),
                })
                .unwrap_or(RuntimeProfileDecision::NoKnownPayloadExtension),
            _ => RuntimeProfileDecision::Ambiguous {
                core_ids: candidates
                    .iter()
                    .map(|candidate| candidate.core.core_id.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            },
        }
    };
    RuntimeProfilePlan {
        game_dir_name: game_dir.name,
        decision,
    }
}

#[cfg(test)]
fn runtime_profile_decision_for_named_candidate(
    game_dir: &catalog_discovery::GameDirHeader,
    candidate: &RuntimeCoreCandidate<'_>,
) -> RuntimeProfileDecision {
    if let Some(extensions) =
        runtime_payload_extensions_for_core_or_dir(&candidate.core.core_id, &game_dir.name)
    {
        if catalog_discovery::game_dir_has_payload_candidate(&game_dir.path, &extensions) {
            return RuntimeProfileDecision::Catalogable {
                profile: Box::new(runtime_profile_for_extensions(
                    game_dir,
                    candidate.core,
                    extensions,
                )),
            };
        }
        let facts = catalog_discovery::game_dir_payload_facts_for_header(game_dir.clone());
        if facts.has_payloadish_files() {
            RuntimeProfileDecision::NoKnownPayloadExtension
        } else {
            RuntimeProfileDecision::EmptyOrMediaOnly
        }
    } else {
        let facts = catalog_discovery::game_dir_payload_facts_for_header(game_dir.clone());
        runtime_profile_for_match(&facts, candidate)
            .map(|profile| RuntimeProfileDecision::Catalogable {
                profile: Box::new(profile),
            })
            .unwrap_or_else(|| {
                if facts.has_payloadish_files() {
                    RuntimeProfileDecision::NoKnownPayloadExtension
                } else {
                    RuntimeProfileDecision::EmptyOrMediaOnly
                }
            })
    }
}

struct RuntimeCoreCandidate<'a> {
    core: &'a catalog_discovery::InstalledCore,
    match_kind: RuntimeCoreMatchKind,
}

#[derive(Clone, Copy)]
enum RuntimeCoreMatchKind {
    Exact,
    Alias,
}

struct RuntimeProfileHint {
    names: &'static [&'static str],
    core_alias: Option<&'static str>,
    extensions: &'static [&'static str],
}

const RUNTIME_PROFILE_HINTS: &[RuntimeProfileHint] = &[
    RuntimeProfileHint {
        names: &["astrocade"],
        core_alias: None,
        extensions: &["bin", "rom"],
    },
    RuntimeProfileHint {
        names: &["atari2600", "atari2600-sinden"],
        core_alias: None,
        extensions: &["a26", "bin"],
    },
    RuntimeProfileHint {
        names: &["atari5200"],
        core_alias: None,
        extensions: &["a52", "bin"],
    },
    RuntimeProfileHint {
        names: &["atari7800"],
        core_alias: None,
        extensions: &["a78", "bin"],
    },
    RuntimeProfileHint {
        names: &["atarilynx"],
        core_alias: None,
        extensions: &["lnx"],
    },
    RuntimeProfileHint {
        names: &["bbcmicro"],
        core_alias: None,
        extensions: &["adl", "dsd", "sdd", "ssd", "uef"],
    },
    RuntimeProfileHint {
        names: &["Coleco"],
        core_alias: Some("ColecoVision"),
        extensions: &["col", "rom"],
    },
    RuntimeProfileHint {
        names: &["fds"],
        core_alias: Some("NES"),
        extensions: &["fds"],
    },
    RuntimeProfileHint {
        names: &["gbc"],
        core_alias: None,
        extensions: &["gbc"],
    },
    RuntimeProfileHint {
        names: &["megaduck"],
        core_alias: None,
        extensions: &["bin"],
    },
    RuntimeProfileHint {
        names: &["Gameboy", "GAMEBOY", "GAMEBOY2P", "Gameboy-Sinden"],
        core_alias: None,
        extensions: &["gb"],
    },
    RuntimeProfileHint {
        names: &["intellivision"],
        core_alias: None,
        extensions: &["int", "rom", "bin"],
    },
    RuntimeProfileHint {
        names: &["ngpc"],
        core_alias: None,
        extensions: &["ngc"],
    },
    RuntimeProfileHint {
        names: &["neogeopocket"],
        core_alias: None,
        extensions: &["ngp"],
    },
    RuntimeProfileHint {
        names: &["s32x"],
        core_alias: None,
        extensions: &["32x"],
    },
    RuntimeProfileHint {
        names: &["sgb2"],
        core_alias: None,
        extensions: &["sfc", "smc"],
    },
    RuntimeProfileHint {
        names: &["satellaview"],
        core_alias: None,
        extensions: &["sfc", "smc", "bs"],
    },
    RuntimeProfileHint {
        names: &["supergrafx", "tgfx16"],
        core_alias: Some("TurboGrafx16"),
        extensions: &["pce"],
    },
    RuntimeProfileHint {
        names: &["tgfx16-cd"],
        core_alias: Some("TurboGrafx16"),
        extensions: &["chd"],
    },
    RuntimeProfileHint {
        names: &["vectrex"],
        core_alias: None,
        extensions: &["vec"],
    },
    RuntimeProfileHint {
        names: &["wonderswan"],
        core_alias: None,
        extensions: &["ws"],
    },
    RuntimeProfileHint {
        names: &["wonderswancolor"],
        core_alias: None,
        extensions: &["wsc"],
    },
    RuntimeProfileHint {
        names: &["Spectrum"],
        core_alias: Some("ZX-Spectrum"),
        extensions: &["sna", "szx", "tap", "tzx", "z80"],
    },
];

fn runtime_core_candidates<'a>(
    game_dir: &catalog_discovery::GameDirFact,
    cores: &'a [catalog_discovery::InstalledCore],
) -> Vec<RuntimeCoreCandidate<'a>> {
    let exact = runtime_core_candidates_by_dir_name(&game_dir.name, cores);
    if !exact.is_empty() || generic_manifest_profile_for_game_dir(&game_dir.name).is_some() {
        return exact;
    }
    unique_extension_core_candidates(game_dir, cores)
}

fn runtime_core_candidates_by_dir_name<'a>(
    game_dir_name: &str,
    cores: &'a [catalog_discovery::InstalledCore],
) -> Vec<RuntimeCoreCandidate<'a>> {
    let exact = core_candidates_by_name(game_dir_name, cores);
    if !exact.is_empty() || generic_manifest_profile_for_game_dir(game_dir_name).is_some() {
        return exact;
    }
    let aliases = core_candidates_by_game_dir_alias(game_dir_name, cores);
    if !aliases.is_empty() {
        return aliases;
    }
    let numeric_family = core_candidates_by_numeric_family_alias(game_dir_name, cores);
    if !numeric_family.is_empty() {
        return numeric_family;
    }
    if let Some(base) = sinden_base_name(game_dir_name) {
        let candidates = core_candidates_by_name(&base, cores)
            .into_iter()
            .map(|mut candidate| {
                candidate.match_kind = RuntimeCoreMatchKind::Alias;
                candidate
            })
            .collect::<Vec<_>>();
        if !candidates.is_empty() {
            return candidates;
        }
    }
    Vec::new()
}

fn core_candidates_by_numeric_family_alias<'a>(
    game_dir_name: &str,
    cores: &'a [catalog_discovery::InstalledCore],
) -> Vec<RuntimeCoreCandidate<'a>> {
    let mut seen = BTreeSet::new();
    cores
        .iter()
        .filter(|core| numeric_family_alias_matches(game_dir_name, &core.core_id))
        .filter_map(|core| {
            seen.insert(core.core_id.to_ascii_lowercase())
                .then_some(RuntimeCoreCandidate {
                    core,
                    match_kind: RuntimeCoreMatchKind::Alias,
                })
        })
        .collect()
}

fn numeric_family_alias_matches(left: &str, right: &str) -> bool {
    fn parts(value: &str) -> Option<(String, String)> {
        let compact = value
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_lowercase())
            .collect::<String>();
        let digit = compact.find(|ch: char| ch.is_ascii_digit())?;
        let (family, model) = compact.split_at(digit);
        (!family.is_empty() && model.len() >= 2 && model.chars().all(|ch| ch.is_ascii_digit()))
            .then(|| (family.to_string(), model.to_string()))
    }
    let Some((left_family, left_model)) = parts(left) else {
        return false;
    };
    let Some((right_family, right_model)) = parts(right) else {
        return false;
    };
    if left_family != right_family {
        return false;
    }
    let (short, long) = if left_model.len() <= right_model.len() {
        (&left_model, &right_model)
    } else {
        (&right_model, &left_model)
    };
    long.len() <= short.len() + 2 && long.starts_with(short)
}

fn core_candidates_by_game_dir_alias<'a>(
    game_dir_name: &str,
    cores: &'a [catalog_discovery::InstalledCore],
) -> Vec<RuntimeCoreCandidate<'a>> {
    let Some(core_id) = runtime_game_dir_core_alias(game_dir_name) else {
        return Vec::new();
    };
    core_candidates_by_name(core_id, cores)
        .into_iter()
        .map(|mut candidate| {
            candidate.match_kind = RuntimeCoreMatchKind::Alias;
            candidate
        })
        .collect()
}

fn runtime_game_dir_core_alias(name: &str) -> Option<&'static str> {
    RUNTIME_PROFILE_HINTS
        .iter()
        .find(|hint| hint_matches_name(hint, name))
        .and_then(|hint| hint.core_alias)
}

fn core_candidates_by_name<'a>(
    name: &str,
    cores: &'a [catalog_discovery::InstalledCore],
) -> Vec<RuntimeCoreCandidate<'a>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for core in cores {
        if !core.core_id.eq_ignore_ascii_case(name)
            && catalog_discovery::compact_system_name(&core.core_id)
                != catalog_discovery::compact_system_name(name)
        {
            continue;
        }
        if !core_path_is_compatible_with_canonical_system(name, &core.path) {
            continue;
        }
        let key = core.core_id.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(RuntimeCoreCandidate {
                core,
                match_kind: RuntimeCoreMatchKind::Exact,
            });
        }
    }
    out
}

fn core_path_is_compatible_with_canonical_system(system: &str, path: &Path) -> bool {
    let normalized = canonical_core_id(system);
    let Some(row) = generic_manifest_rows_cached()
        .iter()
        .find(|row| row.core_name.eq_ignore_ascii_case(&normalized))
    else {
        return true;
    };
    let Some(actual) = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(canonical_core_id)
        .map(|stem| catalog_discovery::compact_system_name(&stem))
    else {
        return false;
    };
    std::iter::once(&row.core_name)
        .chain(row.compatible_core_names.iter())
        .any(|accepted| catalog_discovery::compact_system_name(accepted) == actual)
}

pub fn validate_canonical_core_profile(system_id: &str, core_path: &str) -> Result<(), String> {
    let Some(row) = generic_manifest_rows_cached()
        .iter()
        .find(|row| row.system_id.eq_ignore_ascii_case(system_id))
    else {
        return Ok(());
    };
    if core_path_is_compatible_with_canonical_system(&row.core_name, Path::new(core_path)) {
        Ok(())
    } else {
        let accepted = std::iter::once(row.core_name.as_str())
            .chain(row.compatible_core_names.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "system {system_id} requires one of the compatible cores [{accepted}] but launch plan uses {core_path}"
        ))
    }
}

fn unique_extension_core_candidates<'a>(
    game_dir: &catalog_discovery::GameDirFact,
    cores: &'a [catalog_discovery::InstalledCore],
) -> Vec<RuntimeCoreCandidate<'a>> {
    if game_dir.payload_extensions.is_empty() {
        return Vec::new();
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for core in cores {
        let Some(extensions) = runtime_payload_extensions_for_core(&core.core_id) else {
            continue;
        };
        if !game_dir
            .payload_extensions
            .iter()
            .any(|ext| contains_ignore_ascii_case(&extensions, ext))
        {
            continue;
        }
        let key = core.core_id.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(RuntimeCoreCandidate {
                core,
                match_kind: RuntimeCoreMatchKind::Alias,
            });
        }
    }
    out
}

fn sinden_base_name(name: &str) -> Option<String> {
    let suffix_len = "-sinden".len();
    if name.len() <= suffix_len || !name.to_ascii_lowercase().ends_with("-sinden") {
        return None;
    }
    Some(name[..name.len() - suffix_len].to_string())
}

fn runtime_profile_for_match(
    game_dir: &catalog_discovery::GameDirFact,
    candidate: &RuntimeCoreCandidate<'_>,
) -> Option<LaunchProfile> {
    let core = candidate.core;
    let folder_specific_extensions = (catalog_discovery::compact_system_name(&game_dir.name)
        == "tgfx16cd")
        .then(|| runtime_payload_extension_hints(&game_dir.name))
        .and_then(runtime_extensions_from_iter);
    let extensions = folder_specific_extensions
        .or_else(|| runtime_payload_extensions_for_core_or_dir(&core.core_id, &game_dir.name))
        .or_else(|| {
            matches!(
                candidate.match_kind,
                RuntimeCoreMatchKind::Exact | RuntimeCoreMatchKind::Alias
            )
            .then(|| observed_runtime_payload_extensions(game_dir))
            .filter(|extensions| !extensions.is_empty())
        })
        .or_else(|| {
            game_dir
                .has_zip_files
                .then(|| {
                    catalog_discovery::archive_member_extensions_for_dir(&game_dir.path)
                        .into_iter()
                        .filter(|ext| is_observed_runtime_payload_extension(ext))
                        .collect::<Vec<_>>()
                })
                .filter(|extensions| !extensions.is_empty())
        })?;
    if !game_dir.has_zip_files
        && !game_dir
            .payload_extensions
            .iter()
            .any(|ext| contains_ignore_ascii_case(&extensions, ext))
    {
        return None;
    }

    Some(runtime_profile_for_extensions(
        &catalog_discovery::GameDirHeader {
            name: game_dir.name.clone(),
            path: game_dir.path.clone(),
            signature: game_dir.signature,
            confirmed_directory: false,
        },
        core,
        extensions,
    ))
}

fn runtime_extensions_from_iter(
    extensions: impl IntoIterator<Item = String>,
) -> Option<Vec<String>> {
    runtime_extensions_from_set(extensions.into_iter().collect())
}

fn runtime_profile_for_extensions(
    game_dir: &catalog_discovery::GameDirHeader,
    core: &catalog_discovery::InstalledCore,
    extensions: Vec<String>,
) -> LaunchProfile {
    let mount = if extensions
        .iter()
        .all(|ext| matches!(ext.as_str(), "chd" | "cue" | "vhd"))
    {
        MountSpec::mount_image(0)
    } else {
        MountSpec::load_file(1)
    };
    let payload_rule = PayloadRule {
        extensions,
        mount,
        disposition: PayloadDisposition::Playable,
        provenance: RuleProvenance::conf_str(
            "Runtime top-level games folder matched an installed core with known payload extensions",
        ),
    };
    let core_id = canonical_core_id(&core.core_id).to_ascii_lowercase();
    let distinct_variant = catalog_discovery::compact_system_name(&game_dir.name)
        == catalog_discovery::compact_system_name(&core.core_id)
        || matches!(
            catalog_discovery::compact_system_name(&game_dir.name).as_str(),
            "fds" | "supergrafx" | "tgfx16" | "tgfx16cd"
        );
    let id = if distinct_variant {
        crate::library_db::normalize_id(&game_dir.name)
    } else {
        core_id
    };
    LaunchProfile {
        id: format!("runtime-{id}"),
        category: crate::catalog_classify::platform_kind_for_system(&id)
            .category_label()
            .to_string(),
        system_id: id,
        title: if distinct_variant {
            game_dir.name.clone()
        } else {
            core.core_id.clone()
        },
        core_name: core.core_id.clone(),
        core_path: relative_core_path_for_installed_core(&core.path),
        game_dirs: vec![game_dir.name.clone()],
        payload_rules: vec![payload_rule.clone()],
        archive_entry_rules: vec![payload_rule],
        collection_rules: Vec::new(),
        ignore_rules: vec![IgnoreRule {
            file_names: str_vec(&["boot.rom"]),
            extensions: Vec::new(),
            reason: IgnoreReason::Bios,
            provenance: RuleProvenance::magik(
                "Runtime classification uses boot.rom as firmware evidence, not a game payload",
            ),
        }],
        provenance: RuleProvenance::conf_str(
            "Runtime top-level games folder matched an installed core",
        ),
    }
}

fn runtime_payload_extensions_for_core(core_id: &str) -> Option<Vec<String>> {
    let mut extensions = BTreeSet::new();
    if let Some(profile) = generic_manifest_profile_for_core(core_id) {
        for ext in playable_payload_extensions(&profile) {
            extensions.insert(ext);
        }
    }
    for ext in runtime_payload_extension_hints(core_id) {
        extensions.insert(ext);
    }
    runtime_extensions_from_set(extensions)
}

fn runtime_payload_extensions_for_core_or_dir(
    core_id: &str,
    game_dir: &str,
) -> Option<Vec<String>> {
    let mut extensions = BTreeSet::new();
    if let Some(profile) = generic_manifest_profile_for_core(core_id)
        .or_else(|| generic_manifest_profile_for_game_dir(game_dir))
    {
        for ext in playable_payload_extensions(&profile) {
            extensions.insert(ext);
        }
    }
    for ext in runtime_payload_extension_hints(core_id)
        .into_iter()
        .chain(runtime_payload_extension_hints(game_dir))
    {
        extensions.insert(ext);
    }
    runtime_extensions_from_set(extensions)
}

fn observed_runtime_payload_extensions(game_dir: &catalog_discovery::GameDirFact) -> Vec<String> {
    runtime_extensions_from_set(
        game_dir
            .payload_extensions
            .iter()
            .filter(|ext| is_observed_runtime_payload_extension(ext))
            .cloned()
            .collect(),
    )
    .unwrap_or_default()
}

fn runtime_extensions_from_set(extensions: BTreeSet<String>) -> Option<Vec<String>> {
    if extensions.is_empty() {
        None
    } else {
        Some(extensions.into_iter().collect())
    }
}

fn is_observed_runtime_payload_extension(ext: &str) -> bool {
    !matches!(
        ext.to_ascii_lowercase().as_str(),
        "bak"
            | "cfg"
            | "conf"
            | "dat"
            | "db"
            | "html"
            | "ini"
            | "jpeg"
            | "jpg"
            | "json"
            | "log"
            | "md"
            | "nfo"
            | "pdf"
            | "png"
            | "sav"
            | "sqlite"
            | "srm"
            | "torrent"
            | "txt"
            | "xml"
    )
}

fn playable_payload_extensions(profile: &LaunchProfile) -> Vec<String> {
    let mut extensions = BTreeSet::new();
    for rule in &profile.payload_rules {
        if rule.disposition == PayloadDisposition::Playable {
            for ext in &rule.extensions {
                extensions.insert(ext.to_ascii_lowercase());
            }
        }
    }
    extensions.into_iter().collect()
}

fn runtime_payload_extension_hints(name: &str) -> Vec<String> {
    RUNTIME_PROFILE_HINTS
        .iter()
        .filter(|hint| {
            hint_matches_name(hint, name)
                || hint
                    .core_alias
                    .is_some_and(|core_alias| core_alias.eq_ignore_ascii_case(name))
        })
        .flat_map(|hint| hint.extensions.iter().copied())
        .map(str::to_string)
        .collect()
}

fn hint_matches_name(hint: &RuntimeProfileHint, name: &str) -> bool {
    hint.names
        .iter()
        .any(|hint_name| hint_name.eq_ignore_ascii_case(name))
}

fn relative_core_path_for_installed_core(path: &Path) -> Option<String> {
    let components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let start = components.iter().position(|component| {
        component.eq_ignore_ascii_case("_Console")
            || component.eq_ignore_ascii_case("_Computer")
            || component.eq_ignore_ascii_case("_LLAPI")
            || component.eq_ignore_ascii_case("_Arcade")
    })?;
    let mut relative = components[start..].join("/");
    if relative.to_ascii_lowercase().ends_with(".rbf") {
        relative.truncate(relative.len() - 4);
    }
    Some(relative)
}

pub fn core_launch_manifest_fingerprint() -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in CORE_LAUNCH_MANIFEST_JSON.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn validate_core_launch_manifest() -> Result<(), String> {
    crate::catalog_classify::validate_system_taxonomy()?;
    let manifest = parse_core_launch_manifest()?;
    if manifest.schema != CORE_LAUNCH_MANIFEST_VERSION {
        return Err(format!(
            "core launch manifest schema {} does not match expected {CORE_LAUNCH_MANIFEST_VERSION}",
            manifest.schema
        ));
    }

    let special = special_profiles();
    let missing_systems = special
        .iter()
        .map(|profile| profile.system_id.as_str())
        .chain(manifest.profiles.iter().map(|row| row.system_id.as_str()))
        .filter(|system_id| crate::catalog_classify::system_definition(system_id).is_none())
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    if !missing_systems.is_empty() {
        return Err(format!(
            "checked-in launch profiles missing canonical system taxonomy definitions: {}",
            missing_systems
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let special_ids = special
        .iter()
        .map(|profile| profile.id.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let special_dirs = special
        .iter()
        .flat_map(|profile| profile.game_dirs.iter().map(|dir| dir.to_ascii_lowercase()))
        .collect::<BTreeSet<_>>();
    let mut ids = BTreeSet::new();
    let mut game_dirs = BTreeSet::new();
    for row in &manifest.profiles {
        validate_manifest_text("id", &row.id)?;
        validate_manifest_text("system_id", &row.system_id)?;
        validate_manifest_text("category", &row.category)?;
        validate_manifest_text("title", &row.title)?;
        validate_manifest_text("core_name", &row.core_name)?;
        validate_manifest_text("core_path", &row.core_path)?;
        validate_manifest_text("evidence", &row.evidence)?;
        validate_manifest_compatible_core_names(row)?;
        if row.game_dirs.is_empty() {
            return Err(format!("manifest row {} has no game_dirs", row.id));
        }
        if row.extensions.is_empty() {
            return Err(format!("manifest row {} has no extensions", row.id));
        }
        let id = row.id.to_ascii_lowercase();
        if special_ids.contains(&id) {
            return Err(format!(
                "manifest row {} collides with special profile id",
                row.id
            ));
        }
        if !ids.insert(id) {
            return Err(format!("duplicate manifest profile id {}", row.id));
        }
        for dir in &row.game_dirs {
            validate_manifest_text("game_dir", dir)?;
            let key = dir.to_ascii_lowercase();
            if special_dirs.contains(&key) {
                return Err(format!(
                    "manifest row {} game dir {} collides with special profile",
                    row.id, dir
                ));
            }
            if !game_dirs.insert(key) {
                return Err(format!("duplicate manifest game dir {}", dir));
            }
        }
        for ext in &row.extensions {
            validate_manifest_text("extension", ext)?;
            if ext.contains('.') || ext.contains('/') || ext.contains('\\') {
                return Err(format!(
                    "manifest row {} has invalid extension {}",
                    row.id, ext
                ));
            }
        }
    }
    Ok(())
}

fn validate_manifest_compatible_core_names(row: &CoreLaunchManifestRow) -> Result<(), String> {
    let canonical_core = catalog_discovery::compact_system_name(&row.core_name);
    let mut compatible_cores = BTreeSet::new();
    for compatible_core in &row.compatible_core_names {
        validate_manifest_text("compatible_core_name", compatible_core)?;
        let normalized = catalog_discovery::compact_system_name(compatible_core);
        if normalized.is_empty() {
            return Err(format!(
                "manifest row {} has invalid compatible core {}",
                row.id, compatible_core
            ));
        }
        if normalized == canonical_core {
            return Err(format!(
                "manifest row {} repeats canonical core {} as compatible",
                row.id, compatible_core
            ));
        }
        if !compatible_cores.insert(normalized) {
            return Err(format!(
                "manifest row {} has duplicate compatible core {}",
                row.id, compatible_core
            ));
        }
    }
    Ok(())
}

fn validate_manifest_text(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("manifest field {field} is empty"));
    }
    Ok(())
}

fn all_profiles() -> Vec<LaunchProfile> {
    let mut profiles = special_profiles();
    profiles.extend(generic_manifest_profiles());
    profiles
}

fn special_profiles() -> Vec<LaunchProfile> {
    vec![
        mra_profile(),
        mgl_profile(),
        dos_mgl_profile(),
        neon68k_mgl_profile(),
        saturn_profile(),
        psx_profile(),
        ao486_profile(),
        amiga_profile(),
        neogeo_profile(),
        neogeo_cd_profile(),
        mame_zip_profile(),
        hbmame_zip_profile(),
    ]
}

fn generic_manifest_profiles() -> Vec<LaunchProfile> {
    generic_manifest_profiles_cached().to_vec()
}

fn generic_manifest_profiles_cached() -> &'static [LaunchProfile] {
    static PROFILES: OnceLock<Vec<LaunchProfile>> = OnceLock::new();
    PROFILES
        .get_or_init(|| {
            generic_manifest_rows_cached()
                .iter()
                .cloned()
                .map(generic_manifest_profile)
                .collect()
        })
        .as_slice()
}

fn generic_manifest_rows_cached() -> &'static [CoreLaunchManifestRow] {
    static ROWS: OnceLock<Vec<CoreLaunchManifestRow>> = OnceLock::new();
    ROWS.get_or_init(|| {
        parse_core_launch_manifest()
            .expect("parse core launch manifest")
            .profiles
    })
    .as_slice()
}

fn parse_core_launch_manifest() -> Result<CoreLaunchManifest, String> {
    serde_json::from_str(CORE_LAUNCH_MANIFEST_JSON)
        .map_err(|e| format!("parse core launch manifest: {e}"))
}

fn generic_manifest_profile(row: CoreLaunchManifestRow) -> LaunchProfile {
    let payload_rule = PayloadRule {
        extensions: lower_vec(row.extensions),
        mount: MountSpec::load_file(1),
        disposition: PayloadDisposition::Playable,
        provenance: RuleProvenance::main(row.evidence.clone()),
    };
    LaunchProfile {
        id: row.id,
        system_id: row.system_id,
        category: row.category,
        title: row.title,
        core_name: row.core_name,
        core_path: Some(row.core_path),
        game_dirs: row.game_dirs,
        payload_rules: vec![payload_rule.clone()],
        archive_entry_rules: if row.archive_entries {
            vec![payload_rule]
        } else {
            Vec::new()
        },
        collection_rules: Vec::new(),
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::main(
            "Generated MagiK core launch manifest row backed by Main file-picker behavior",
        ),
    }
}

fn mra_profile() -> LaunchProfile {
    LaunchProfile {
        id: "mra".into(),
        system_id: "arcade".into(),
        category: "Arcade".into(),
        title: "MRA Launcher".into(),
        core_name: "MRA".into(),
        core_path: None,
        game_dirs: str_vec(&["_Arcade"]),
        payload_rules: vec![launcher_payload_rule()],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: vec![IgnoreRule {
            file_names: str_vec(&["NeoGeo Pocket.mra", "NeoGeo Pocket Color.mra"]),
            extensions: Vec::new(),
            reason: IgnoreReason::Tool,
            provenance: RuleProvenance::magik(
                "NeoGeo Pocket MRAs boot a handheld system core rather than an arcade game",
            ),
        }],
        provenance: RuleProvenance::mra("Main mra_loader parses .mra as launch XML"),
    }
}

fn mgl_profile() -> LaunchProfile {
    LaunchProfile {
        id: "mgl".into(),
        system_id: "launcher".into(),
        category: "Launcher".into(),
        title: "MGL Launcher".into(),
        core_name: "MGL".into(),
        core_path: None,
        game_dirs: str_vec(&["_Games", "_Console (autoboot)"]),
        payload_rules: vec![launcher_payload_rule()],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::mgl("Main mra_loader parses .mgl file mount actions"),
    }
}

fn dos_mgl_profile() -> LaunchProfile {
    LaunchProfile {
        id: "dos".into(),
        system_id: "dos".into(),
        category: "Computer".into(),
        title: "DOS Games".into(),
        core_name: "AO486".into(),
        core_path: Some("_Computer/AO486".into()),
        game_dirs: str_vec(&["_DOS Games"]),
        payload_rules: vec![launcher_payload_rule()],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::mgl("Installed DOS game launchers live under _DOS Games"),
    }
}

fn neon68k_mgl_profile() -> LaunchProfile {
    LaunchProfile {
        id: "neon68k".into(),
        system_id: "x68000".into(),
        category: "Computer".into(),
        title: "X68000 Games".into(),
        core_name: "X68000".into(),
        core_path: Some("_Computer/X68000".into()),
        game_dirs: str_vec(&["_X68000 Games", "X68000 Games"]),
        payload_rules: vec![launcher_payload_rule()],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::mgl("Installed X68000 game launchers use per-game MGL files"),
    }
}

fn saturn_profile() -> LaunchProfile {
    LaunchProfile {
        id: "saturn".into(),
        system_id: "saturn".into(),
        category: "Console".into(),
        title: "Saturn".into(),
        core_name: "Saturn".into(),
        core_path: Some("_Console/Saturn".into()),
        game_dirs: str_vec(&["Saturn"]),
        payload_rules: vec![PayloadRule {
            extensions: str_vec(&["cue", "chd"]),
            mount: MountSpec::mount_image(0),
            disposition: PayloadDisposition::Playable,
            provenance: RuleProvenance::main("support/saturn/saturncdd.cpp accepts .cue and .chd"),
        }],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: vec![
            IgnoreRule {
                file_names: str_vec(&["boot.rom", "cd_bios.rom"]),
                extensions: Vec::new(),
                reason: IgnoreReason::Bios,
                provenance: RuleProvenance::main(
                    "support/saturn/saturn.cpp loads boot.rom/cd_bios.rom as BIOS",
                ),
            },
            IgnoreRule {
                file_names: Vec::new(),
                extensions: str_vec(&["bin", "img"]),
                reason: IgnoreReason::CueTrack,
                provenance: RuleProvenance::main(
                    "support/saturn/saturncdd.cpp resolves CUE track files from the CUE",
                ),
            },
        ],
        provenance: RuleProvenance::main("Main has Saturn-specific image handling in menu.cpp"),
    }
}

fn psx_profile() -> LaunchProfile {
    LaunchProfile {
        id: "psx".into(),
        system_id: "psx".into(),
        category: "Console".into(),
        title: "PlayStation".into(),
        core_name: "PSX".into(),
        core_path: Some("_Console/PSX".into()),
        game_dirs: str_vec(&["PSX"]),
        payload_rules: vec![PayloadRule {
            extensions: str_vec(&["cue", "chd"]),
            mount: MountSpec::mount_image(1),
            disposition: PayloadDisposition::Playable,
            provenance: RuleProvenance::main(
                "menu.cpp routes PSX disc images through psx_mount_cd",
            ),
        }],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: vec![
            IgnoreRule {
                file_names: str_vec(&["boot.rom", "boot1.rom", "boot2.rom"]),
                extensions: Vec::new(),
                reason: IgnoreReason::Bios,
                provenance: RuleProvenance::main(
                    "PSX boot ROMs live under games/PSX as support files",
                ),
            },
            IgnoreRule {
                file_names: Vec::new(),
                extensions: str_vec(&["bin", "img"]),
                reason: IgnoreReason::CueTrack,
                provenance: RuleProvenance::main(
                    "PSX CUE descriptors reference BIN/IMG tracks as dependent media",
                ),
            },
            IgnoreRule {
                file_names: str_vec(&["sbi.zip"]),
                extensions: str_vec(&["sbi"]),
                reason: IgnoreReason::SupportArchive,
                provenance: RuleProvenance::main("PSX SBI data is auxiliary disc metadata"),
            },
        ],
        provenance: RuleProvenance::main("Main detects PSX by core name and has PSX CD handling"),
    }
}

fn ao486_profile() -> LaunchProfile {
    LaunchProfile {
        id: "ao486".into(),
        system_id: "ao486".into(),
        category: "Computer".into(),
        title: "AO486".into(),
        core_name: "AO486".into(),
        core_path: Some("_Computer/AO486".into()),
        game_dirs: str_vec(&["AO486"]),
        payload_rules: vec![PayloadRule {
            extensions: str_vec(&["vhd", "chd", "cue", "iso", "img"]),
            mount: MountSpec::mount_image(2),
            disposition: PayloadDisposition::AttachedMedia,
            provenance: RuleProvenance::mgl(
                "AO486 game MGLs attach disk media to slots rather than making raw media primary games",
            ),
        }],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: vec![IgnoreRule {
            file_names: str_vec(&["boot0.rom", "boot1.rom", "boot1_opensource.rom"]),
            extensions: Vec::new(),
            reason: IgnoreReason::Bios,
            provenance: RuleProvenance::main("AO486 boot ROMs are support files under games/AO486"),
        }],
        provenance: RuleProvenance::main(
            "Main detects AO486 by core name and routes image mounts through x86_set_image",
        ),
    }
}

fn amiga_profile() -> LaunchProfile {
    LaunchProfile {
        id: "amiga".into(),
        system_id: "amiga".into(),
        category: "Computer".into(),
        title: "Amiga".into(),
        core_name: "Minimig".into(),
        core_path: Some("_Computer/Minimig".into()),
        game_dirs: str_vec(&["Amiga"]),
        payload_rules: vec![
            PayloadRule {
                extensions: str_vec(&["adf"]),
                mount: MountSpec::mount_image(0),
                disposition: PayloadDisposition::Playable,
                provenance: RuleProvenance::main(
                    "menu.cpp Minimig floppy picker sets fs_pFileExt=ADF",
                ),
            },
            PayloadRule {
                extensions: str_vec(&["hdf", "vhd", "img", "dsk", "iso", "cue", "chd"]),
                mount: MountSpec::mount_image(0),
                disposition: PayloadDisposition::Playable,
                provenance: RuleProvenance::main(
                    "menu.cpp Minimig hardfile picker accepts HDF/VHD/IMG/DSK and ISO/CUE/CHD media",
                ),
            },
        ],
        archive_entry_rules: Vec::new(),
        collection_rules: vec![CollectionRule {
            archive_extensions: str_vec(&["7z"]),
            file_name_contains: str_vec(&["amigavision"]),
            listings: vec![
                collection_listing("games/Amiga/listings/games.txt", "AmigaVision"),
                collection_listing("games/Amiga/listings/demos.txt", "AmigaVision demos"),
            ],
            provenance: RuleProvenance::magik(
                "Explicit MagiK profile for AmigaVision archives: listings/games.txt and demos.txt enumerate launchable titles",
            ),
        }],
        ignore_rules: vec![IgnoreRule {
            file_names: str_vec(&[
                "kick.rom",
                "kick13.rom",
                "kick20.rom",
                "kick31.rom",
                "kickstart.rom",
                "hrtmon.rom",
            ]),
            extensions: Vec::new(),
            reason: IgnoreReason::Bios,
            provenance: RuleProvenance::main(
                "support/minimig/minimig_config.cpp loads Kickstart/HRTmon ROMs as support files",
            ),
        }],
        provenance: RuleProvenance::main(
            "Main identifies the Minimig core and exposes Amiga floppy/hardfile media pickers",
        ),
    }
}

fn neogeo_profile() -> LaunchProfile {
    let neo_rule = PayloadRule {
        extensions: str_vec(&["neo"]),
        mount: MountSpec::load_file(1),
        disposition: PayloadDisposition::Playable,
        provenance: RuleProvenance::mgl(
            "Installed NeoGeo organizer MGLs use type=f index=1 for .neo payloads inside ZIPs",
        ),
    };

    LaunchProfile {
        id: "neogeo".into(),
        system_id: "neogeo".into(),
        category: "Arcade".into(),
        title: "NeoGeo".into(),
        core_name: "NeoGeo".into(),
        core_path: Some("_Console/NeoGeo".into()),
        game_dirs: str_vec(&["NEOGEO", "NeoGeo"]),
        payload_rules: vec![
            PayloadRule {
                extensions: str_vec(&["zip"]),
                mount: MountSpec::load_file(1),
                disposition: PayloadDisposition::Playable,
                provenance: RuleProvenance::main(
                    "menu.cpp enables SCANO_NEOGEO; file_io.cpp treats .zip sets as selectable NeoGeo entries",
                ),
            },
            neo_rule.clone(),
        ],
        archive_entry_rules: vec![neo_rule],
        collection_rules: Vec::new(),
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::main(
            "Main routes NeoGeo file selection through SCANO_NEOGEO and neogeo_romset_tx",
        ),
    }
}

fn neogeo_cd_profile() -> LaunchProfile {
    LaunchProfile {
        id: "neogeo-cd".into(),
        system_id: "neogeo-cd".into(),
        category: "Console".into(),
        title: "NeoGeo CD".into(),
        core_name: "NeoGeo".into(),
        core_path: Some("_Console/NeoGeo".into()),
        game_dirs: str_vec(&["NeoGeo-CD"]),
        payload_rules: vec![PayloadRule {
            extensions: str_vec(&["cue", "chd"]),
            mount: MountSpec::mount_image(0),
            disposition: PayloadDisposition::Playable,
            provenance: RuleProvenance::main(
                "menu.cpp routes NeoGeo CD cue/chd image selection through NeoGeo-CD and neocd_set_image",
            ),
        }],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: vec![
            IgnoreRule {
                file_names: Vec::new(),
                extensions: str_vec(&["bin"]),
                reason: IgnoreReason::CueTrack,
                provenance: RuleProvenance::main(
                    "NeoGeo CD CUE files reference BIN tracks as support media",
                ),
            },
            IgnoreRule {
                file_names: Vec::new(),
                extensions: str_vec(&["rom"]),
                reason: IgnoreReason::Bios,
                provenance: RuleProvenance::main(
                    "NeoGeo CD ROM files under the game directory are BIOS/support payloads",
                ),
            },
        ],
        provenance: RuleProvenance::main(
            "Main has NeoGeo CD-specific image handling for the NeoGeo core",
        ),
    }
}

fn mame_zip_profile() -> LaunchProfile {
    arcade_zip_set_profile(
        "mame",
        "MAME Zip Sets",
        "mame",
        "Raw MAME zip sets are folded into existing Arcade MRA launch targets when their set names match",
    )
}

fn hbmame_zip_profile() -> LaunchProfile {
    arcade_zip_set_profile(
        "hbmame",
        "HBMAME Zip Sets",
        "hbmame",
        "Raw HBMAME zip sets are folded into existing Arcade MRA launch targets when their set names match",
    )
}

fn arcade_zip_set_profile(
    id: &str,
    title: &str,
    game_dir: &str,
    provenance: &str,
) -> LaunchProfile {
    LaunchProfile {
        id: id.into(),
        system_id: "arcade".into(),
        category: "Arcade".into(),
        title: title.into(),
        core_name: "Arcade".into(),
        core_path: None,
        game_dirs: str_vec(&[game_dir]),
        payload_rules: vec![PayloadRule {
            extensions: str_vec(&["zip"]),
            mount: MountSpec::load_file(1),
            disposition: PayloadDisposition::Playable,
            provenance: RuleProvenance::magik(provenance),
        }],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::magik(provenance),
    }
}

fn launcher_payload_rule() -> PayloadRule {
    PayloadRule {
        extensions: str_vec(&["mra", "mgl"]),
        mount: MountSpec::launcher(),
        disposition: PayloadDisposition::Playable,
        provenance: RuleProvenance::mgl("MRA/MGL files are launcher descriptors loaded by Main"),
    }
}

fn collection_listing(entry_path: &str, genre: &str) -> CollectionListing {
    CollectionListing {
        entry_path: entry_path.to_string(),
        genre: genre.to_string(),
    }
}

fn str_vec(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn lower_vec(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

fn contains_ignore_ascii_case(items: &[String], needle: &str) -> bool {
    items.iter().any(|item| item.eq_ignore_ascii_case(needle))
}

fn path_ext(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
}

pub(crate) fn canonical_core_id(stem: &str) -> String {
    let mut core = stem;
    if let Some((prefix, suffix)) = stem.rsplit_once('_')
        && suffix.len() == 8
        && suffix.chars().all(|c| c.is_ascii_digit())
    {
        core = prefix;
    }
    core.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn numeric_family_aliases_require_matching_family_and_close_model_prefix() {
        assert!(numeric_family_alias_matches("PC8801", "PC88"));
        assert!(numeric_family_alias_matches("pc-88", "PC8801"));
        assert!(!numeric_family_alias_matches("Atari7800", "Atari5200"));
        assert!(!numeric_family_alias_matches("PC88", "PC9801"));
        assert!(!numeric_family_alias_matches("Loose88", "PC88"));
        assert!(!numeric_family_alias_matches("X1", "X68000"));
    }
    use crate::test_support::unique_temp_dir;

    #[test]
    fn manifest_rows_validate_and_do_not_collide_with_special_profiles() {
        validate_core_launch_manifest().expect("valid generated core launch manifest");
        let atari2600 = generic_manifest_rows_cached()
            .iter()
            .find(|row| row.system_id == "atari2600")
            .expect("Atari 2600 manifest row");
        assert_eq!(atari2600.compatible_core_names, ["Atari7800"]);
        let gbc = generic_manifest_rows_cached()
            .iter()
            .find(|row| row.system_id == "gbc")
            .expect("GBC manifest row");
        assert_eq!(gbc.compatible_core_names, ["Gameboy"]);
    }

    #[test]
    fn manifest_compatible_core_names_are_normalized_unique_and_not_canonical() {
        let mut row = generic_manifest_rows_cached()
            .iter()
            .find(|row| row.system_id == "atari2600")
            .expect("Atari 2600 manifest row")
            .clone();
        row.compatible_core_names = vec!["Atari 7800".into(), "atari7800".into()];
        assert!(
            validate_manifest_compatible_core_names(&row)
                .expect_err("normalized duplicate")
                .contains("duplicate compatible core")
        );

        row.compatible_core_names = vec!["Atari2600".into()];
        assert!(
            validate_manifest_compatible_core_names(&row)
                .expect_err("canonical duplicate")
                .contains("repeats canonical core")
        );

        row.compatible_core_names = vec!["---".into()];
        assert!(
            validate_manifest_compatible_core_names(&row)
                .expect_err("empty normalized core")
                .contains("invalid compatible core")
        );
    }

    #[test]
    fn active_profiles_require_installed_generic_core() {
        let root = unique_temp_dir("active-profiles-installed-core");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");

        let no_core = active_profiles_for_roots(&[root.display().to_string()]);
        assert!(profile_for_game_dir(&no_core, "ColecoVision").is_none());

        std::fs::write(root.join("_Console/ColecoVision_20260603.rbf"), b"rbf")
            .expect("write core");
        let with_core = active_profiles_for_roots(&[root.display().to_string()]);
        let coleco =
            profile_for_game_dir(&with_core, "ColecoVision").expect("colecovision profile");
        assert_eq!(coleco.system_id, "colecovision");
        assert!(
            coleco
                .classify_archive_entry(Path::new("Smurf Rescue.col"))
                .is_some()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn canonical_profile_keeps_logical_core_path_with_multiple_dated_versions() {
        let root = unique_temp_dir("canonical-profile-logical-core");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/SNES")).expect("create games dir");
        // Create the newer file first so the assertion cannot depend on read_dir order.
        std::fs::write(root.join("_Console/SNES_20260603.rbf"), b"new").expect("write new core");
        std::fs::write(root.join("_Console/SNES_20240408.rbf"), b"old").expect("write old core");
        std::fs::write(root.join("games/SNES/ActRaiser.sfc"), b"rom").expect("write game");

        let profiles = active_profiles_for_roots(&[root.display().to_string()]);
        let snes = profile_for_game_dir(&profiles, "SNES").expect("SNES profile");

        assert_eq!(snes.core_path.as_deref(), Some("_Console/SNES"));
        assert_eq!(snes.payload_rules[0].mount, MountSpec::load_file(1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn catalog_scan_plan_keeps_unclaimed_headers_and_finalizes_from_collected_facts() {
        let root = unique_temp_dir("catalog-scan-plan");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/NES")).expect("create nes dir");
        std::fs::create_dir_all(root.join("games/Loose")).expect("create loose dir");
        std::fs::write(root.join("_Console/NES_20260630.rbf"), b"rbf").expect("write nes core");
        std::fs::write(root.join("_Console/Gameboy_20260630.rbf"), b"rbf")
            .expect("write gameboy core");
        std::fs::write(root.join("games/Loose/Tetris.gb"), b"rom").expect("write rom");

        let roots = vec![root.display().to_string()];
        let plan = CatalogScanPlan::for_roots(&roots);

        assert!(
            plan.installed_cores()
                .iter()
                .any(|core| core.core_id == "NES")
        );
        assert!(
            plan.base_profiles()
                .iter()
                .any(|profile| profile.system_id == "nes")
        );
        assert!(
            plan.game_dir_headers()
                .iter()
                .any(|header| header.name == "Loose")
        );
        assert!(
            !plan
                .game_dir_headers()
                .iter()
                .any(|header| header.name.eq_ignore_ascii_case("NES"))
        );

        let game_dirs = plan
            .game_dir_headers()
            .iter()
            .cloned()
            .map(catalog_discovery::game_dir_payload_facts_for_header)
            .collect::<Vec<_>>();
        let loose_fact = game_dirs
            .iter()
            .find(|facts| facts.name == "Loose")
            .expect("loose facts");
        let resolved = plan
            .profile_for_game_dir_facts(loose_fact)
            .expect("profile for supplied facts");
        assert_eq!(resolved.id, "runtime-gameboy");
        assert_eq!(resolved.game_dirs, vec!["Loose"]);
        let expected_profiles = active_profiles_for_roots(&roots);
        std::fs::remove_dir_all(root.join("games/Loose")).expect("remove walked dir");

        let profiles = plan.finalize_profiles(&game_dirs);
        let gameboy = profiles
            .iter()
            .find(|profile| profile.id == "runtime-gameboy")
            .expect("runtime gameboy profile from collected facts");
        assert_eq!(gameboy.game_dirs, vec!["Loose"]);
        assert_eq!(profiles, expected_profiles);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_planner_catalogs_exact_gameboy_payloads() {
        let root = unique_temp_dir("runtime-plan-gameboy");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/Gameboy")).expect("create gameboy dir");
        std::fs::write(root.join("_Console/Gameboy_20260630.rbf"), b"rbf").expect("write core");
        std::fs::write(root.join("games/Gameboy/Tetris.gb"), b"rom").expect("write rom");

        let plans = runtime_profile_plans_for_roots(&[root.display().to_string()]);
        let plan = plans
            .iter()
            .find(|plan| plan.game_dir_name == "Gameboy")
            .expect("gameboy plan");

        let RuntimeProfileDecision::Catalogable { profile } = &plan.decision else {
            panic!("expected catalogable gameboy plan, got {:?}", plan.decision);
        };
        assert_eq!(profile.system_id, "gameboy");
        assert_eq!(profile.game_dirs, vec!["Gameboy"]);
        assert_eq!(playable_payload_extensions(profile), vec!["gb"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_planner_catalogs_case_insensitive_exact_matches() {
        let root = unique_temp_dir("runtime-plan-gameboy-case");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/gameboy")).expect("create gameboy dir");
        std::fs::write(root.join("_Console/Gameboy_20260630.rbf"), b"rbf").expect("write core");
        std::fs::write(root.join("games/gameboy/Tetris.gb"), b"rom").expect("write rom");

        let plans = runtime_profile_plans_for_roots(&[root.display().to_string()]);
        let plan = plans
            .iter()
            .find(|plan| plan.game_dir_name == "gameboy")
            .expect("gameboy plan");

        let RuntimeProfileDecision::Catalogable { profile } = &plan.decision else {
            panic!(
                "expected catalogable case-insensitive gameboy plan, got {:?}",
                plan.decision
            );
        };
        assert_eq!(profile.system_id, "gameboy");
        assert_eq!(profile.game_dirs, vec!["gameboy"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_planner_catalogs_unique_sinden_suffix_aliases() {
        let root = unique_temp_dir("runtime-plan-gameboy-sinden");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/Gameboy-Sinden")).expect("create gameboy dir");
        std::fs::write(root.join("_Console/Gameboy_20260630.rbf"), b"rbf").expect("write core");
        std::fs::write(root.join("games/Gameboy-Sinden/Tetris.gb"), b"rom").expect("write rom");

        let plans = runtime_profile_plans_for_roots(&[root.display().to_string()]);
        let plan = plans
            .iter()
            .find(|plan| plan.game_dir_name == "Gameboy-Sinden")
            .expect("gameboy-sinden plan");

        let RuntimeProfileDecision::Catalogable { profile } = &plan.decision else {
            panic!(
                "expected catalogable sinden gameboy plan, got {:?}",
                plan.decision
            );
        };
        assert_eq!(profile.system_id, "gameboy");
        assert_eq!(profile.game_dirs, vec!["Gameboy-Sinden"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn active_runtime_profiles_have_unique_ids_for_multiple_alias_dirs() {
        let root = unique_temp_dir("active-runtime-profile-ids");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/Gameboy")).expect("create gameboy dir");
        std::fs::create_dir_all(root.join("games/Gameboy-Sinden")).expect("create sinden dir");
        std::fs::write(root.join("_Console/Gameboy_20260630.rbf"), b"rbf").expect("write core");
        std::fs::write(root.join("games/Gameboy/Tetris.gb"), b"rom").expect("write rom");
        std::fs::write(root.join("games/Gameboy-Sinden/Tetris.gb"), b"rom").expect("write rom");

        let profiles = active_profiles_for_roots(&[root.display().to_string()]);
        let ids = profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>();
        let unique_ids = ids.iter().copied().collect::<BTreeSet<_>>();

        assert_eq!(ids.len(), unique_ids.len(), "{ids:?}");
        let gameboy_profiles = profiles
            .iter()
            .filter(|profile| profile.id == "runtime-gameboy")
            .collect::<Vec<_>>();
        assert_eq!(gameboy_profiles.len(), 1, "{ids:?}");
        let game_dirs = gameboy_profiles[0]
            .game_dirs
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(game_dirs, BTreeSet::from(["Gameboy", "Gameboy-Sinden"]));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn launch_target_profile_lookup_handles_runtime_profile_ids() {
        let root = unique_temp_dir("runtime-profile-launch-target-lookup");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/Intellivision"))
            .expect("create intellivision dir");
        std::fs::write(root.join("_Console/Intellivision_20260630.rbf"), b"rbf")
            .expect("write intellivision core");
        std::fs::write(root.join("games/Intellivision/Armor Battle.int"), b"rom")
            .expect("write intellivision rom");

        let plans = runtime_profile_plans_for_roots(&[root.display().to_string()]);
        let plan = plans
            .iter()
            .find(|plan| plan.game_dir_name == "Intellivision")
            .expect("intellivision plan");

        let RuntimeProfileDecision::Catalogable { profile } = &plan.decision else {
            panic!(
                "expected catalogable intellivision plan, got {:?}",
                plan.decision
            );
        };
        let profiles = vec![profile.as_ref().clone()];

        let profile = profile_for_launch_target_id(&profiles, "intellivision")
            .expect("runtime profile matched by system id");

        assert_eq!(profile.id, "runtime-intellivision");
        assert_eq!(profile.system_id, "intellivision");
        assert_eq!(profile.payload_rules[0].mount, MountSpec::load_file(1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_planner_resolves_coleco_alias_without_ambiguous_extension_leak() {
        let root = unique_temp_dir("runtime-plan-coleco-alias");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/Coleco")).expect("create coleco dir");
        std::fs::write(root.join("_Console/ColecoVision_20260630.rbf"), b"rbf")
            .expect("write coleco core");
        std::fs::write(root.join("_Console/Atari5200_20260630.rbf"), b"rbf")
            .expect("write atari core");
        std::fs::write(root.join("_Console/Intellivision_20260630.rbf"), b"rbf")
            .expect("write intellivision core");
        std::fs::write(root.join("games/Coleco/Smurf Rescue.col"), b"rom").expect("write rom");

        let plans = runtime_profile_plans_for_roots(&[root.display().to_string()]);
        let plan = plans
            .iter()
            .find(|plan| plan.game_dir_name == "Coleco")
            .expect("coleco plan");

        let RuntimeProfileDecision::Catalogable { profile } = &plan.decision else {
            panic!("expected catalogable coleco plan, got {:?}", plan.decision);
        };
        assert_eq!(profile.system_id, "colecovision");
        assert_eq!(profile.game_dirs, vec!["Coleco"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_planner_resolves_spectrum_alias_to_zx_spectrum_core() {
        let root = unique_temp_dir("runtime-plan-spectrum-alias");
        std::fs::create_dir_all(root.join("_Computer")).expect("create computer dir");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/Spectrum")).expect("create spectrum dir");
        std::fs::write(root.join("_Computer/ZX-Spectrum_20260630.rbf"), b"rbf")
            .expect("write spectrum core");
        std::fs::write(root.join("_Console/ColecoVision_20260630.rbf"), b"rbf")
            .expect("write coleco core");
        std::fs::write(root.join("_Console/Intellivision_20260630.rbf"), b"rbf")
            .expect("write intellivision core");
        std::fs::write(root.join("games/Spectrum/Jet Set Willy.tzx"), b"tape")
            .expect("write spectrum tape");
        std::fs::write(root.join("games/Spectrum/support.rom"), b"rom").expect("write support rom");

        let plans = runtime_profile_plans_for_roots(&[root.display().to_string()]);
        let plan = plans
            .iter()
            .find(|plan| plan.game_dir_name == "Spectrum")
            .expect("spectrum plan");

        let RuntimeProfileDecision::Catalogable { profile } = &plan.decision else {
            panic!(
                "expected catalogable spectrum plan, got {:?}",
                plan.decision
            );
        };
        assert_eq!(profile.system_id, "zx-spectrum");
        assert_eq!(profile.title, "ZX-Spectrum");
        assert_eq!(profile.category, "Computer");
        assert_eq!(profile.game_dirs, vec!["Spectrum"]);
        assert_eq!(profile.payload_rules[0].mount, MountSpec::load_file(1));
        assert_eq!(
            playable_payload_extensions(profile),
            vec!["sna", "szx", "tap", "tzx", "z80"]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_planner_keeps_unprofiled_cd_aliases_unsupported() {
        let root = unique_temp_dir("runtime-plan-unprofiled-cd");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/TGFX16-CD")).expect("create tgfx dir");
        std::fs::create_dir_all(root.join("games/MegaCD")).expect("create megacd dir");
        std::fs::write(root.join("_Console/ColecoVision_20260630.rbf"), b"rbf")
            .expect("write coleco core");
        std::fs::write(root.join("_Console/SMS_20260630.rbf"), b"rbf").expect("write sms core");
        std::fs::write(root.join("games/TGFX16-CD/Dracula X.chd"), b"disc")
            .expect("write tgfx disc");
        std::fs::write(root.join("games/MegaCD/Sonic CD.chd"), b"disc").expect("write mega disc");

        let plans = runtime_profile_plans_for_roots(&[root.display().to_string()]);
        let tgfx = plans
            .iter()
            .find(|plan| plan.game_dir_name == "TGFX16-CD")
            .expect("tgfx plan");
        let megacd = plans
            .iter()
            .find(|plan| plan.game_dir_name == "MegaCD")
            .expect("megacd plan");

        assert_eq!(tgfx.decision, RuntimeProfileDecision::NoInstalledCore);
        assert_eq!(megacd.decision, RuntimeProfileDecision::NoInstalledCore);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_planner_marks_shared_extension_aliases_ambiguous() {
        let root = unique_temp_dir("runtime-plan-ambiguous-sg");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/Loose")).expect("create loose dir");
        std::fs::write(root.join("_Console/ColecoVision_20260630.rbf"), b"rbf")
            .expect("write coleco core");
        std::fs::write(root.join("_Console/SMS_20260630.rbf"), b"rbf").expect("write sms core");
        std::fs::write(root.join("games/Loose/Zaxxon.sg"), b"rom").expect("write sg rom");

        let plans = runtime_profile_plans_for_roots(&[root.display().to_string()]);
        let plan = plans
            .iter()
            .find(|plan| plan.game_dir_name == "Loose")
            .expect("loose plan");

        assert_eq!(
            plan.decision,
            RuntimeProfileDecision::Ambiguous {
                core_ids: vec!["ColecoVision".to_string(), "SMS".to_string()]
            }
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_planner_derives_exact_core_payload_extensions() {
        let root = unique_temp_dir("runtime-plan-derived-c64");
        std::fs::create_dir_all(root.join("_Computer")).expect("create computer dir");
        std::fs::create_dir_all(root.join("games/C64")).expect("create c64 dir");
        std::fs::write(root.join("_Computer/C64_20260630.rbf"), b"rbf").expect("write core");
        std::fs::write(root.join("games/C64/Impossible Mission.d64"), b"disk").expect("write disk");
        std::fs::write(root.join("games/C64/metadata.xml"), b"xml").expect("write metadata");
        std::fs::write(root.join("games/C64/cover.png"), b"png").expect("write image");

        let plans = runtime_profile_plans_for_roots(&[root.display().to_string()]);
        let plan = plans
            .iter()
            .find(|plan| plan.game_dir_name == "C64")
            .expect("c64 plan");

        let RuntimeProfileDecision::Catalogable { profile } = &plan.decision else {
            panic!("expected catalogable c64 plan, got {:?}", plan.decision);
        };
        assert_eq!(profile.system_id, "c64");
        assert_eq!(profile.category, "Computer");
        assert_eq!(playable_payload_extensions(profile), vec!["d64"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_planner_keeps_aliases_without_known_payload_rule_unsupported() {
        let root = unique_temp_dir("runtime-plan-alias-unknown-ext");
        std::fs::create_dir_all(root.join("_Computer")).expect("create computer dir");
        std::fs::create_dir_all(root.join("games/Computer")).expect("create computer dir");
        std::fs::write(root.join("_Computer/C64_20260630.rbf"), b"rbf").expect("write core");
        std::fs::write(root.join("games/Computer/Impossible Mission.d64"), b"disk")
            .expect("write disk");

        let plans = runtime_profile_plans_for_roots(&[root.display().to_string()]);
        let plan = plans
            .iter()
            .find(|plan| plan.game_dir_name == "Computer")
            .expect("computer plan");

        assert_eq!(plan.decision, RuntimeProfileDecision::NoInstalledCore);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_planner_rejects_empty_exact_game_folders() {
        let root = unique_temp_dir("runtime-plan-empty");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/Gameboy")).expect("create gameboy dir");
        std::fs::write(root.join("_Console/Gameboy_20260630.rbf"), b"rbf").expect("write core");

        let plans = runtime_profile_plans_for_roots(&[root.display().to_string()]);
        let plan = plans
            .iter()
            .find(|plan| plan.game_dir_name == "Gameboy")
            .expect("gameboy plan");

        assert_eq!(plan.decision, RuntimeProfileDecision::EmptyOrMediaOnly);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn saturn_profile_launches_only_cue_and_chd_disc_images() {
        let profiles = builtin_profiles();
        let saturn = profile_for_game_dir(&profiles, "Saturn").expect("saturn profile");

        assert!(matches!(
            saturn.classify_path(Path::new("/media/fat/games/Saturn/Astal (US).chd")),
            ProfilePathClass::Payload {
                rule: PayloadRule {
                    disposition: PayloadDisposition::Playable,
                    mount: MountSpec {
                        kind: MountKind::MountImage,
                        index: 0,
                        ..
                    },
                    ..
                }
            }
        ));
        assert!(matches!(
            saturn.classify_path(Path::new("/media/fat/games/Saturn/Croc.cue")),
            ProfilePathClass::Payload { .. }
        ));
        assert_eq!(
            saturn.classify_path(Path::new("/media/fat/games/Saturn/boot.rom")),
            ProfilePathClass::Ignored {
                reason: IgnoreReason::Bios,
                provenance: RuleProvenance::main(
                    "support/saturn/saturn.cpp loads boot.rom/cd_bios.rom as BIOS"
                )
            }
        );
        assert_eq!(
            saturn.classify_path(Path::new("/media/fat/games/Saturn/track01.img")),
            ProfilePathClass::Ignored {
                reason: IgnoreReason::CueTrack,
                provenance: RuleProvenance::main(
                    "support/saturn/saturncdd.cpp resolves CUE track files from the CUE"
                )
            }
        );
    }

    #[test]
    fn ao486_media_is_attached_not_primary_playable_content() {
        let profiles = builtin_profiles();
        let ao486 = profile_for_game_dir(&profiles, "AO486").expect("ao486 profile");

        assert!(matches!(
            ao486.classify_path(Path::new("/media/fat/games/AO486/media/doom/doom.vhd")),
            ProfilePathClass::Payload {
                rule: PayloadRule {
                    disposition: PayloadDisposition::AttachedMedia,
                    ..
                }
            }
        ));
        assert!(matches!(
            ao486.classify_path(Path::new("/media/fat/games/AO486/boot1.rom")),
            ProfilePathClass::Ignored {
                reason: IgnoreReason::Bios,
                ..
            }
        ));
    }

    #[test]
    fn common_cartridge_profiles_are_resolved_by_game_dir() {
        let profiles = builtin_profiles();
        let snes = profile_for_game_dir(&profiles, "SNES").expect("snes profile");

        assert_eq!(snes.system_id, "snes");
        assert!(matches!(
            snes.classify_path(Path::new("/media/fat/games/SNES/ActRaiser.sfc")),
            ProfilePathClass::Payload {
                rule: PayloadRule {
                    disposition: PayloadDisposition::Playable,
                    mount: MountSpec {
                        kind: MountKind::LoadFile,
                        index: 1,
                        ..
                    },
                    ..
                }
            }
        ));
        assert!(
            snes.classify_archive_entry(Path::new("ActRaiser.sfc"))
                .is_some()
        );

        let wonderswan_color =
            profile_for_game_dir(&profiles, "WonderSwanColor").expect("wonderswan profile");
        assert_eq!(wonderswan_color.id, "wonderswan");
        assert_eq!(wonderswan_color.system_id, "wonderswan");
        assert!(
            wonderswan_color
                .classify_archive_entry(Path::new("Gunpey EX.wsc"))
                .is_some()
        );
        assert!(matches!(
            wonderswan_color
                .classify_path(Path::new("/media/fat/games/WonderSwanColor/Gunpey EX.wsc")),
            ProfilePathClass::Payload {
                rule: PayloadRule {
                    disposition: PayloadDisposition::Playable,
                    mount: MountSpec {
                        kind: MountKind::LoadFile,
                        index: 1,
                        ..
                    },
                    ..
                }
            }
        ));
    }

    #[test]
    fn generic_content_roles_exclude_support_material() {
        let profile = runtime_profile_for_extensions(
            &catalog_discovery::GameDirHeader {
                name: "SupportCore".to_string(),
                path: PathBuf::from("/media/fat/games/SupportCore"),
                signature: catalog_discovery::GameDirSignature::Unavailable,
                confirmed_directory: false,
            },
            &catalog_discovery::InstalledCore {
                core_id: "SupportCore".to_string(),
                path: PathBuf::from("/media/fat/_Computer/SupportCore.rbf"),
            },
            str_vec(&["cas", "jce", "nvr", "rom", "sh"]),
        );

        for path in [
            "boot0.rom",
            "1.3.rom",
            "Kickstart-1.3-r34.5.rom",
            "firmware_bios.rom",
            "machine.nvr",
            "Palettes/BlackGreenBlueCyan.GBP",
            "ntsc.act",
            "eeprom.jce",
            "bin2dsk.sh",
            "BlankTape.cas",
            "Demos/Actually Named Like A Game.adf",
            "Example Demo.cas",
        ] {
            assert!(matches!(
                profile.classify_path(Path::new(path)),
                ProfilePathClass::Ignored { .. }
            ));
        }
        assert!(matches!(
            profile.classify_path(Path::new("Real Game.cas")),
            ProfilePathClass::Payload { .. }
        ));
    }

    #[test]
    fn runtime_core_location_never_decides_product_classification() {
        for (name, expected) in [
            ("SMS", "Console"),
            ("GameGear", "Handheld"),
            ("Astrocade", "Console"),
        ] {
            let profile = runtime_profile_for_extensions(
                &catalog_discovery::GameDirHeader {
                    name: name.to_string(),
                    path: PathBuf::from(format!("/media/fat/games/{name}")),
                    signature: catalog_discovery::GameDirSignature::Unavailable,
                    confirmed_directory: false,
                },
                &catalog_discovery::InstalledCore {
                    core_id: name.to_string(),
                    path: PathBuf::from(format!("/media/fat/_Arcade/cores/{name}.rbf")),
                },
                str_vec(&["rom"]),
            );
            assert_eq!(profile.category, expected, "{name}");
        }
    }

    #[test]
    fn mgl_setname_creates_distinct_system_on_shared_core() {
        let root = unique_temp_dir("runtime-mgl-system");
        std::fs::create_dir_all(root.join("_Console")).expect("create console");
        std::fs::create_dir_all(root.join("games/GBC")).expect("create games");
        std::fs::write(root.join("_Console/Gameboy_20260630.rbf"), b"rbf").expect("write core");
        std::fs::write(
            root.join("_Console/GameboyColor.mgl"),
            r#"<mistergamedescription><rbf>_Console/Gameboy</rbf><setname>GBC</setname></mistergamedescription>"#,
        )
        .expect("write descriptor");
        std::fs::write(root.join("games/GBC/Zelda.gbc"), b"rom").expect("write game");

        let profiles = active_profiles_for_roots(&[root.display().to_string()]);
        let profile = profile_for_game_dir(&profiles, "GBC").expect("GBC profile");

        assert_eq!(profile.system_id, "gbc");
        assert!(
            profile
                .core_path
                .as_deref()
                .is_some_and(|path| path.contains("Gameboy"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_compatible_descriptor_activates_atari_2600_through_atari_7800() {
        let root = unique_temp_dir("runtime-mgl-compatible-core");
        std::fs::create_dir_all(root.join("_Console")).expect("create console");
        std::fs::create_dir_all(root.join("games/Atari2600")).expect("create games");
        std::fs::write(root.join("_Console/Atari7800_20260630.rbf"), b"rbf").expect("write core");
        std::fs::write(root.join("games/Atari2600/Adventure.a26"), b"rom").expect("write game");
        std::fs::write(root.join("games/Atari2600/Stray.a78"), b"rom").expect("write stray");

        let without_descriptor = active_profiles_for_roots(&[root.display().to_string()]);
        assert!(
            profile_for_game_dir(&without_descriptor, "Atari2600").is_none(),
            "a compatible physical core alone must not infer another system"
        );

        std::fs::write(
            root.join("_Console/Atari 2600.mgl"),
            r#"<mistergamedescription><rbf>_Console/Atari7800</rbf><setname>Atari2600</setname></mistergamedescription>"#,
        )
        .expect("write descriptor");

        let profiles = active_profiles_for_roots(&[root.display().to_string()]);
        let profile =
            profile_for_game_dir(&profiles, "Atari2600").expect("Atari 2600 shared-core profile");
        assert_eq!(profile.system_id, "atari2600");
        assert!(
            profile
                .core_path
                .as_deref()
                .is_some_and(|path| path.contains("Atari7800"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn descriptor_cannot_advertise_canonical_system_through_undeclared_core() {
        let root = unique_temp_dir("runtime-mgl-undeclared-core");
        std::fs::create_dir_all(root.join("_Console")).expect("create console");
        std::fs::create_dir_all(root.join("games/Atari2600")).expect("create games");
        std::fs::write(root.join("_Console/Atari5200_20260630.rbf"), b"rbf").expect("write core");
        std::fs::write(
            root.join("_Console/Atari 2600.mgl"),
            r#"<mistergamedescription><rbf>_Console/Atari5200</rbf><setname>Atari2600</setname></mistergamedescription>"#,
        )
        .expect("write descriptor");
        std::fs::write(root.join("games/Atari2600/Adventure.a26"), b"rom").expect("write game");

        let profiles = active_profiles_for_roots(&[root.display().to_string()]);
        assert!(
            profile_for_game_dir(&profiles, "Atari2600").is_none(),
            "profiles: {profiles:#?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn canonical_core_overrides_descriptor_pointing_at_another_system() {
        let root = unique_temp_dir("runtime-mgl-canonical-core");
        std::fs::create_dir_all(root.join("_Console")).expect("create console");
        std::fs::create_dir_all(root.join("games/Atari2600")).expect("create games");
        std::fs::write(root.join("_Console/Atari2600_20260630.rbf"), b"rbf")
            .expect("write Atari 2600 core");
        std::fs::write(root.join("_Console/Atari7800_20260630.rbf"), b"rbf")
            .expect("write Atari 7800 core");
        std::fs::write(
            root.join("_Console/Atari 2600.mgl"),
            r#"<mistergamedescription><rbf>_Console/Atari7800</rbf><setname>Atari2600</setname></mistergamedescription>"#,
        )
        .expect("write descriptor");

        let profiles = active_profiles_for_roots(&[root.display().to_string()]);
        let profile = profile_for_game_dir(&profiles, "Atari2600").expect("Atari 2600 profile");

        assert_eq!(profile.system_id, "atari2600");
        assert!(
            profile
                .core_path
                .as_deref()
                .is_some_and(|path| path.contains("Atari2600"))
        );
        assert!(
            !profile
                .core_path
                .as_deref()
                .is_some_and(|path| path.contains("Atari7800"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dos_mgl_profile_is_separate_from_generic_mgl_launchers() {
        let profiles = builtin_profiles();
        let dos = profile_for_game_dir(&profiles, "_DOS Games").expect("dos profile");
        let mgl = profile_for_game_dir(&profiles, "_Games").expect("mgl profile");

        assert_eq!(dos.id, "dos");
        assert_eq!(dos.system_id, "dos");
        assert_eq!(dos.category, "Computer");
        assert_eq!(mgl.id, "mgl");
        assert_eq!(mgl.system_id, "launcher");
    }

    #[test]
    fn neogeo_profile_accepts_compressed_romsets_and_neo_entries() {
        let profiles = builtin_profiles();
        let neogeo = profile_for_game_dir(&profiles, "NEOGEO").expect("neogeo profile");

        assert!(matches!(
            neogeo.classify_path(Path::new(
                "/media/fat/games/NEOGEO/Neo Geo Mister FGPA Ultra Pack.zip"
            )),
            ProfilePathClass::Payload { .. }
        ));
        assert!(matches!(
            neogeo.classify_path(Path::new("/media/fat/games/NEOGEO/mslug3.neo")),
            ProfilePathClass::Payload {
                rule: PayloadRule {
                    disposition: PayloadDisposition::Playable,
                    mount: MountSpec {
                        kind: MountKind::LoadFile,
                        index: 1,
                        ..
                    },
                    ..
                }
            }
        ));
    }

    #[test]
    fn neogeo_cd_profile_launches_disc_images_and_ignores_support_files() {
        let profiles = builtin_profiles();
        let neogeo_cd = profile_for_game_dir(&profiles, "NeoGeo-CD").expect("neogeo-cd profile");

        assert_eq!(neogeo_cd.id, "neogeo-cd");
        assert_eq!(neogeo_cd.system_id, "neogeo-cd");
        assert!(matches!(
            neogeo_cd.classify_path(Path::new("/media/fat/games/NeoGeo-CD/Last Blade.chd")),
            ProfilePathClass::Payload {
                rule: PayloadRule {
                    disposition: PayloadDisposition::Playable,
                    mount: MountSpec {
                        kind: MountKind::MountImage,
                        index: 0,
                        ..
                    },
                    ..
                }
            }
        ));
        assert!(matches!(
            neogeo_cd.classify_path(Path::new("/media/fat/games/NeoGeo-CD/Viewpoint.cue")),
            ProfilePathClass::Payload { .. }
        ));
        assert!(matches!(
            neogeo_cd.classify_path(Path::new("/media/fat/games/NeoGeo-CD/track01.bin")),
            ProfilePathClass::Ignored {
                reason: IgnoreReason::CueTrack,
                ..
            }
        ));
        assert!(matches!(
            neogeo_cd.classify_path(Path::new("/media/fat/games/NeoGeo-CD/neocd.rom")),
            ProfilePathClass::Ignored {
                reason: IgnoreReason::Bios,
                ..
            }
        ));
    }

    #[test]
    fn amiga_profile_accepts_minimig_media_and_amigavision_collection() {
        let profiles = builtin_profiles();
        let amiga = profile_for_game_dir(&profiles, "Amiga").expect("amiga profile");

        assert!(matches!(
            amiga.classify_path(Path::new("/media/fat/games/Amiga/WheelDriverAkiko.adf")),
            ProfilePathClass::Payload {
                rule: PayloadRule {
                    disposition: PayloadDisposition::Playable,
                    mount: MountSpec {
                        kind: MountKind::MountImage,
                        index: 0,
                        ..
                    },
                    ..
                }
            }
        ));
        assert!(matches!(
            amiga.classify_path(Path::new(
                "/media/fat/games/Amiga/AmigaVision-MiSTer-2026.04.26.7z"
            )),
            ProfilePathClass::Collection { .. }
        ));
    }

    #[test]
    fn every_builtin_rule_has_provenance() {
        for profile in builtin_profiles() {
            assert!(!profile.provenance.detail.is_empty(), "{}", profile.id);
            for rule in profile.payload_rules {
                assert!(!rule.provenance.detail.is_empty(), "{}", profile.id);
            }
            for rule in profile.ignore_rules {
                assert!(!rule.provenance.detail.is_empty(), "{}", profile.id);
            }
        }
    }
}
