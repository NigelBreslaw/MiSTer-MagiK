//! Source-derived launch profiles for MiSTer library scanning.
//!
//! This module is intentionally data-oriented. Platform behavior should be
//! represented as a profile with provenance instead of hidden scanner branches.

use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleSourceKind {
    MainSource,
    Mgl,
    Mra,
    ConfStr,
    MagikProfile,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuleProvenance {
    pub kind: RuleSourceKind,
    pub detail: &'static str,
}

impl RuleProvenance {
    pub const fn main(detail: &'static str) -> Self {
        Self {
            kind: RuleSourceKind::MainSource,
            detail,
        }
    }

    pub const fn mgl(detail: &'static str) -> Self {
        Self {
            kind: RuleSourceKind::Mgl,
            detail,
        }
    }

    pub const fn mra(detail: &'static str) -> Self {
        Self {
            kind: RuleSourceKind::Mra,
            detail,
        }
    }

    pub const fn magik(detail: &'static str) -> Self {
        Self {
            kind: RuleSourceKind::MagikProfile,
            detail,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IgnoreReason {
    Bios,
    CueTrack,
    CoreBinary,
    SaveMedia,
    SupportArchive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PayloadRule {
    pub extensions: &'static [&'static str],
    pub mount: MountSpec,
    pub disposition: PayloadDisposition,
    pub provenance: RuleProvenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IgnoreRule {
    pub file_names: &'static [&'static str],
    pub extensions: &'static [&'static str],
    pub reason: IgnoreReason,
    pub provenance: RuleProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchProfile {
    pub id: &'static str,
    pub system_id: &'static str,
    pub title: &'static str,
    pub core_name: &'static str,
    pub game_dirs: Vec<&'static str>,
    pub payload_rules: Vec<PayloadRule>,
    pub ignore_rules: Vec<IgnoreRule>,
    pub provenance: RuleProvenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfilePathClass {
    Payload {
        rule: PayloadRule,
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
                    provenance: rule.provenance,
                };
            }
        }

        let ext = path_ext(path);
        for rule in &self.payload_rules {
            if ext
                .as_deref()
                .is_some_and(|ext| contains_ignore_ascii_case(rule.extensions, ext))
            {
                return ProfilePathClass::Payload { rule: *rule };
            }
        }

        ProfilePathClass::NotMatched
    }
}

impl IgnoreRule {
    fn matches(&self, path: &Path) -> bool {
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if contains_ignore_ascii_case(self.file_names, file_name) {
            return true;
        }

        path_ext(path)
            .as_deref()
            .is_some_and(|ext| contains_ignore_ascii_case(self.extensions, ext))
    }
}

pub fn builtin_profiles() -> Vec<LaunchProfile> {
    vec![
        mra_profile(),
        mgl_profile(),
        saturn_profile(),
        psx_profile(),
        ao486_profile(),
        cartridge_profile("nes", "nes", "NES", "NES", &["NES"], &["nes", "fds"]),
        cartridge_profile("snes", "snes", "SNES", "SNES", &["SNES"], &["sfc", "smc"]),
        cartridge_profile("gba", "gba", "GBA", "GBA", &["GBA"], &["gba"]),
        cartridge_profile("gbc", "gbc", "Game Boy Color", "GBC", &["GBC"], &["gbc"]),
        cartridge_profile(
            "gamegear",
            "gamegear",
            "Game Gear",
            "GameGear",
            &["GameGear"],
            &["gg"],
        ),
        cartridge_profile(
            "megadrive",
            "megadrive",
            "Mega Drive",
            "MegaDrive",
            &["MegaDrive"],
            &["md", "gen"],
        ),
        cartridge_profile(
            "n64",
            "n64",
            "Nintendo 64",
            "N64",
            &["N64"],
            &["n64", "z64", "v64"],
        ),
    ]
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

fn mra_profile() -> LaunchProfile {
    LaunchProfile {
        id: "mra",
        system_id: "arcade",
        title: "MRA Launcher",
        core_name: "MRA",
        game_dirs: vec!["_Arcade"],
        payload_rules: vec![launcher_payload_rule()],
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::mra("Main mra_loader parses .mra as launch XML"),
    }
}

fn mgl_profile() -> LaunchProfile {
    LaunchProfile {
        id: "mgl",
        system_id: "launcher",
        title: "MGL Launcher",
        core_name: "MGL",
        game_dirs: vec!["_Games", "_DOS Games", "_Console (autoboot)"],
        payload_rules: vec![launcher_payload_rule()],
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::mgl("Main mra_loader parses .mgl file mount actions"),
    }
}

fn saturn_profile() -> LaunchProfile {
    LaunchProfile {
        id: "saturn",
        system_id: "saturn",
        title: "Saturn",
        core_name: "Saturn",
        game_dirs: vec!["Saturn"],
        payload_rules: vec![PayloadRule {
            extensions: &["cue", "chd"],
            mount: MountSpec::mount_image(0),
            disposition: PayloadDisposition::Playable,
            provenance: RuleProvenance::main("support/saturn/saturncdd.cpp accepts .cue and .chd"),
        }],
        ignore_rules: vec![
            IgnoreRule {
                file_names: &["boot.rom", "cd_bios.rom"],
                extensions: &[],
                reason: IgnoreReason::Bios,
                provenance: RuleProvenance::main(
                    "support/saturn/saturn.cpp loads boot.rom/cd_bios.rom as BIOS",
                ),
            },
            IgnoreRule {
                file_names: &[],
                extensions: &["bin", "img"],
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
        id: "psx",
        system_id: "psx",
        title: "PlayStation",
        core_name: "PSX",
        game_dirs: vec!["PSX"],
        payload_rules: vec![PayloadRule {
            extensions: &["cue", "chd"],
            mount: MountSpec::mount_image(1),
            disposition: PayloadDisposition::Playable,
            provenance: RuleProvenance::main(
                "menu.cpp routes PSX disc images through psx_mount_cd",
            ),
        }],
        ignore_rules: vec![
            IgnoreRule {
                file_names: &["boot.rom", "boot1.rom", "boot2.rom"],
                extensions: &[],
                reason: IgnoreReason::Bios,
                provenance: RuleProvenance::main(
                    "PSX boot ROMs live under games/PSX as support files",
                ),
            },
            IgnoreRule {
                file_names: &["sbi.zip"],
                extensions: &["sbi"],
                reason: IgnoreReason::SupportArchive,
                provenance: RuleProvenance::main("PSX SBI data is auxiliary disc metadata"),
            },
        ],
        provenance: RuleProvenance::main("Main detects PSX by core name and has PSX CD handling"),
    }
}

fn ao486_profile() -> LaunchProfile {
    LaunchProfile {
        id: "ao486",
        system_id: "ao486",
        title: "AO486",
        core_name: "AO486",
        game_dirs: vec!["AO486"],
        payload_rules: vec![PayloadRule {
            extensions: &["vhd", "chd", "cue", "iso", "img"],
            mount: MountSpec::mount_image(2),
            disposition: PayloadDisposition::AttachedMedia,
            provenance: RuleProvenance::mgl(
                "AO486 game MGLs attach disk media to slots rather than making raw media primary games",
            ),
        }],
        ignore_rules: vec![IgnoreRule {
            file_names: &["boot0.rom", "boot1.rom", "boot1_opensource.rom"],
            extensions: &[],
            reason: IgnoreReason::Bios,
            provenance: RuleProvenance::main(
                "AO486 boot ROMs are support files under games/AO486",
            ),
        }],
        provenance: RuleProvenance::main("Main detects AO486 by core name and routes image mounts through x86_set_image"),
    }
}

fn cartridge_profile(
    id: &'static str,
    system_id: &'static str,
    title: &'static str,
    core_name: &'static str,
    game_dirs: &'static [&'static str],
    extensions: &'static [&'static str],
) -> LaunchProfile {
    LaunchProfile {
        id,
        system_id,
        title,
        core_name,
        game_dirs: game_dirs.to_vec(),
        payload_rules: vec![PayloadRule {
            extensions,
            mount: MountSpec::load_file(1),
            disposition: PayloadDisposition::Playable,
            provenance: RuleProvenance::mgl(
                "Existing organizer MGLs use type=f index=1 for cartridge payloads",
            ),
        }],
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::magik(
            "Explicit MagiK profile derived from Main game paths and installed MGL examples",
        ),
    }
}

fn launcher_payload_rule() -> PayloadRule {
    PayloadRule {
        extensions: &["mra", "mgl"],
        mount: MountSpec::launcher(),
        disposition: PayloadDisposition::Playable,
        provenance: RuleProvenance::mgl("MRA/MGL files are launcher descriptors loaded by Main"),
    }
}

fn contains_ignore_ascii_case(items: &[&str], needle: &str) -> bool {
    items.iter().any(|item| item.eq_ignore_ascii_case(needle))
}

fn path_ext(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

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
