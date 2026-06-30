//! Source-derived launch profiles for MiSTer library scanning.
//!
//! Profiles are built from explicit special cases plus a generated manifest of
//! generic Main file-picker cores. Runtime scans activate manifest rows only
//! when the matching core is installed, so game folders alone do not become
//! launchable by guesswork.

use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const PROFILE_SET_VERSION: u32 = 3;
pub const CORE_LAUNCH_MANIFEST_VERSION: u32 = 1;

const CORE_LAUNCH_MANIFEST_JSON: &str = include_str!("../data/core_launch_manifest.json");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleSourceKind {
    MainSource,
    Mgl,
    Mra,
    ConfStr,
    MagikProfile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

    pub fn magik(detail: impl Into<String>) -> Self {
        Self {
            kind: RuleSourceKind::MagikProfile,
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MountKind {
    Launcher,
    LoadFile,
    MountImage,
    Core,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

impl LaunchProfile {
    pub fn classify_path(&self, path: &Path) -> ProfilePathClass {
        for rule in &self.ignore_rules {
            if rule.matches(path) {
                return ProfilePathClass::Ignored {
                    reason: rule.reason,
                    provenance: rule.provenance.clone(),
                };
            }
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
        let ext = path_ext(path)?;
        self.archive_entry_rules
            .iter()
            .find(|rule| contains_ignore_ascii_case(&rule.extensions, &ext))
            .cloned()
    }
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

#[derive(Debug, Deserialize)]
struct CoreLaunchManifestRow {
    id: String,
    system_id: String,
    category: String,
    title: String,
    core_name: String,
    core_path: String,
    game_dirs: Vec<String>,
    extensions: Vec<String>,
    archive_entries: bool,
    evidence: String,
}

pub fn builtin_profiles() -> Vec<LaunchProfile> {
    all_profiles()
}

pub fn active_profiles_for_roots(roots: &[String]) -> Vec<LaunchProfile> {
    let installed = installed_core_ids_for_roots(roots);
    let mut profiles = special_profiles();
    profiles.extend(generic_manifest_profiles().into_iter().filter(|profile| {
        installed.contains(&canonical_core_id(&profile.core_name).to_ascii_lowercase())
    }));
    profiles
}

pub fn generic_manifest_profile_for_game_dir(game_dir: &str) -> Option<LaunchProfile> {
    generic_manifest_profiles().into_iter().find(|profile| {
        profile
            .game_dirs
            .iter()
            .any(|dir| dir.eq_ignore_ascii_case(game_dir))
    })
}

pub fn generic_manifest_profile_for_core(core_id: &str) -> Option<LaunchProfile> {
    let normalized = canonical_core_id(core_id);
    generic_manifest_profiles()
        .into_iter()
        .find(|profile| profile.core_name.eq_ignore_ascii_case(&normalized))
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
    let mut out = BTreeSet::new();
    for search_root in core_search_roots(roots) {
        if !search_root.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(search_root)
            .follow_links(false)
            .max_depth(3)
            .into_iter()
            .filter_entry(|entry| !should_ignore_hidden_path(entry.path()))
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if !entry.file_type().is_file() || !path_ext_eq(path, "rbf") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem.eq_ignore_ascii_case("menu") {
                continue;
            }
            out.insert(canonical_core_id(stem).to_ascii_lowercase());
        }
    }
    out
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
    let manifest = parse_core_launch_manifest()?;
    if manifest.schema != CORE_LAUNCH_MANIFEST_VERSION {
        return Err(format!(
            "core launch manifest schema {} does not match expected {CORE_LAUNCH_MANIFEST_VERSION}",
            manifest.schema
        ));
    }

    let special = special_profiles();
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
        if row.game_dirs.is_empty() {
            return Err(format!("manifest row {} has no game_dirs", row.id));
        }
        if row.extensions.is_empty() {
            return Err(format!("manifest row {} has no extensions", row.id));
        }
        let id = row.id.to_ascii_lowercase();
        if special_ids.contains(&id) {
            return Err(format!("manifest row {} collides with special profile id", row.id));
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
        saturn_profile(),
        psx_profile(),
        ao486_profile(),
        amiga_profile(),
        neogeo_profile(),
    ]
}

fn generic_manifest_profiles() -> Vec<LaunchProfile> {
    parse_core_launch_manifest()
        .expect("parse core launch manifest")
        .profiles
        .into_iter()
        .map(generic_manifest_profile)
        .collect()
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
        ignore_rules: Vec::new(),
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
            provenance: RuleProvenance::main(
                "AO486 boot ROMs are support files under games/AO486",
            ),
        }],
        provenance: RuleProvenance::main("Main detects AO486 by core name and routes image mounts through x86_set_image"),
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

fn core_search_roots(roots: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for root in roots {
        let root = Path::new(root);
        let candidates = if path_name_eq(root, "games") {
            let base = root.parent().unwrap_or(root);
            vec![
                base.join("_Console"),
                base.join("_Computer"),
                base.join("_Arcade/cores"),
                base.join("_LLAPI"),
            ]
        } else if path_name_eq(root, "_Arcade") {
            vec![root.join("cores")]
        } else if path_name_eq(root, "_Console")
            || path_name_eq(root, "_Computer")
            || path_name_eq(root, "_LLAPI")
        {
            vec![root.to_path_buf()]
        } else {
            vec![
                root.join("_Console"),
                root.join("_Computer"),
                root.join("_Arcade/cores"),
                root.join("_LLAPI"),
            ]
        };
        for candidate in candidates {
            let key = candidate.display().to_string().to_ascii_lowercase();
            if seen.insert(key) {
                out.push(candidate);
            }
        }
    }
    out
}

pub(crate) fn canonical_core_id(stem: &str) -> String {
    let mut core = stem;
    if let Some((prefix, suffix)) = stem.rsplit_once('_') {
        if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
            core = prefix;
        }
    }
    core.to_string()
}

fn path_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

fn path_ext_eq(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(expected))
}

fn should_ignore_hidden_path(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name.len() > 1 && name.starts_with('.')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_temp_dir;

    #[test]
    fn manifest_rows_validate_and_do_not_collide_with_special_profiles() {
        validate_core_launch_manifest().expect("valid generated core launch manifest");
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
        assert!(coleco
            .classify_archive_entry(Path::new("Smurf Rescue.col"))
            .is_some());
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
        assert!(snes
            .classify_archive_entry(Path::new("ActRaiser.sfc"))
            .is_some());

        let wonderswan_color =
            profile_for_game_dir(&profiles, "WonderSwanColor").expect("wonderswan profile");
        assert_eq!(wonderswan_color.id, "wonderswan");
        assert_eq!(wonderswan_color.system_id, "wonderswan");
        assert!(wonderswan_color
            .classify_archive_entry(Path::new("Gunpey EX.wsc"))
            .is_some());
        assert!(matches!(
            wonderswan_color.classify_path(Path::new(
                "/media/fat/games/WonderSwanColor/Gunpey EX.wsc"
            )),
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
