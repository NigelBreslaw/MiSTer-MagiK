//! Source-derived launch profiles for MiSTer library scanning.
//!
//! This module is intentionally data-oriented. Platform behavior should be
//! represented as a profile with provenance instead of hidden scanner branches.

use std::path::Path;

pub const PROFILE_SET_VERSION: u32 = 1;

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
pub struct CollectionListing {
    pub entry_path: &'static str,
    pub genre: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CollectionRule {
    pub archive_extensions: &'static [&'static str],
    pub file_name_contains: &'static [&'static str],
    pub listings: &'static [CollectionListing],
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
    pub category: &'static str,
    pub title: &'static str,
    pub core_name: &'static str,
    pub core_path: Option<&'static str>,
    pub game_dirs: Vec<&'static str>,
    pub payload_rules: Vec<PayloadRule>,
    pub archive_entry_rules: Vec<PayloadRule>,
    pub collection_rules: Vec<CollectionRule>,
    pub ignore_rules: Vec<IgnoreRule>,
    pub provenance: RuleProvenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
                    provenance: rule.provenance,
                };
            }
        }

        for rule in &self.collection_rules {
            if rule.matches(path) {
                return ProfilePathClass::Collection { rule: *rule };
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

    pub fn classify_archive_entry(&self, path: &Path) -> Option<PayloadRule> {
        let ext = path_ext(path)?;
        self.archive_entry_rules
            .iter()
            .find(|rule| contains_ignore_ascii_case(rule.extensions, &ext))
            .copied()
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

impl CollectionRule {
    fn matches(&self, path: &Path) -> bool {
        let Some(ext) = path_ext(path) else {
            return false;
        };
        if !contains_ignore_ascii_case(self.archive_extensions, &ext) {
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

pub fn builtin_profiles() -> Vec<LaunchProfile> {
    vec![
        mra_profile(),
        mgl_profile(),
        dos_mgl_profile(),
        saturn_profile(),
        psx_profile(),
        ao486_profile(),
        amiga_profile(),
        neogeo_profile(),
        cartridge_profile(CartridgeProfileSpec {
            id: "nes",
            system_id: "nes",
            category: "Console",
            title: "NES",
            core_name: "NES",
            core_path: "_Console/NES",
            game_dirs: &["NES"],
            extensions: &["nes", "fds"],
        }),
        cartridge_profile(CartridgeProfileSpec {
            id: "snes",
            system_id: "snes",
            category: "Console",
            title: "SNES",
            core_name: "SNES",
            core_path: "_Console/SNES",
            game_dirs: &["SNES"],
            extensions: &["sfc", "smc"],
        }),
        cartridge_profile(CartridgeProfileSpec {
            id: "gba",
            system_id: "gba",
            category: "Handheld",
            title: "GBA",
            core_name: "GBA",
            core_path: "_Console/GBA",
            game_dirs: &["GBA"],
            extensions: &["gba"],
        }),
        cartridge_profile(CartridgeProfileSpec {
            id: "gbc",
            system_id: "gbc",
            category: "Handheld",
            title: "Game Boy Color",
            core_name: "GBC",
            core_path: "_Console/GBC",
            game_dirs: &["GBC"],
            extensions: &["gbc"],
        }),
        cartridge_profile(CartridgeProfileSpec {
            id: "gamegear",
            system_id: "gamegear",
            category: "Handheld",
            title: "Game Gear",
            core_name: "GameGear",
            core_path: "_Console/GameGear",
            game_dirs: &["GameGear"],
            extensions: &["gg"],
        }),
        cartridge_profile(CartridgeProfileSpec {
            id: "sms",
            system_id: "sms",
            category: "Console",
            title: "Sega Master System",
            core_name: "SMS",
            core_path: "_Console/SMS",
            game_dirs: &["SMS"],
            extensions: &["sms", "sg"],
        }),
        cartridge_profile(CartridgeProfileSpec {
            id: "megadrive",
            system_id: "megadrive",
            category: "Console",
            title: "Mega Drive",
            core_name: "MegaDrive",
            core_path: "_Console/MegaDrive",
            game_dirs: &["MegaDrive"],
            extensions: &["md", "gen"],
        }),
        cartridge_profile(CartridgeProfileSpec {
            id: "n64",
            system_id: "n64",
            category: "Console",
            title: "Nintendo 64",
            core_name: "N64",
            core_path: "_Console/N64",
            game_dirs: &["N64"],
            extensions: &["n64", "z64", "v64"],
        }),
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
        category: "Arcade",
        title: "MRA Launcher",
        core_name: "MRA",
        core_path: None,
        game_dirs: vec!["_Arcade"],
        payload_rules: vec![launcher_payload_rule()],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::mra("Main mra_loader parses .mra as launch XML"),
    }
}

fn mgl_profile() -> LaunchProfile {
    LaunchProfile {
        id: "mgl",
        system_id: "launcher",
        category: "Launcher",
        title: "MGL Launcher",
        core_name: "MGL",
        core_path: None,
        game_dirs: vec!["_Games", "_Console (autoboot)"],
        payload_rules: vec![launcher_payload_rule()],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::mgl("Main mra_loader parses .mgl file mount actions"),
    }
}

fn dos_mgl_profile() -> LaunchProfile {
    LaunchProfile {
        id: "dos",
        system_id: "dos",
        category: "Computer",
        title: "DOS Games",
        core_name: "AO486",
        core_path: Some("_Computer/AO486"),
        game_dirs: vec!["_DOS Games"],
        payload_rules: vec![launcher_payload_rule()],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::mgl("Installed DOS game launchers live under _DOS Games"),
    }
}

fn saturn_profile() -> LaunchProfile {
    LaunchProfile {
        id: "saturn",
        system_id: "saturn",
        category: "Console",
        title: "Saturn",
        core_name: "Saturn",
        core_path: Some("_Console/Saturn"),
        game_dirs: vec!["Saturn"],
        payload_rules: vec![PayloadRule {
            extensions: &["cue", "chd"],
            mount: MountSpec::mount_image(0),
            disposition: PayloadDisposition::Playable,
            provenance: RuleProvenance::main("support/saturn/saturncdd.cpp accepts .cue and .chd"),
        }],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
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
        category: "Console",
        title: "PlayStation",
        core_name: "PSX",
        core_path: Some("_Console/PSX"),
        game_dirs: vec!["PSX"],
        payload_rules: vec![PayloadRule {
            extensions: &["cue", "chd"],
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
        category: "Computer",
        title: "AO486",
        core_name: "AO486",
        core_path: Some("_Computer/AO486"),
        game_dirs: vec!["AO486"],
        payload_rules: vec![PayloadRule {
            extensions: &["vhd", "chd", "cue", "iso", "img"],
            mount: MountSpec::mount_image(2),
            disposition: PayloadDisposition::AttachedMedia,
            provenance: RuleProvenance::mgl(
                "AO486 game MGLs attach disk media to slots rather than making raw media primary games",
            ),
        }],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
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

fn amiga_profile() -> LaunchProfile {
    LaunchProfile {
        id: "amiga",
        system_id: "amiga",
        category: "Computer",
        title: "Amiga",
        core_name: "Minimig",
        core_path: Some("_Computer/Minimig"),
        game_dirs: vec!["Amiga"],
        payload_rules: vec![
            PayloadRule {
                extensions: &["adf"],
                mount: MountSpec::mount_image(0),
                disposition: PayloadDisposition::Playable,
                provenance: RuleProvenance::main(
                    "menu.cpp Minimig floppy picker sets fs_pFileExt=ADF",
                ),
            },
            PayloadRule {
                extensions: &["hdf", "vhd", "img", "dsk", "iso", "cue", "chd"],
                mount: MountSpec::mount_image(0),
                disposition: PayloadDisposition::Playable,
                provenance: RuleProvenance::main(
                    "menu.cpp Minimig hardfile picker accepts HDF/VHD/IMG/DSK and ISO/CUE/CHD media",
                ),
            },
        ],
        archive_entry_rules: Vec::new(),
        collection_rules: vec![CollectionRule {
            archive_extensions: &["7z"],
            file_name_contains: &["amigavision"],
            listings: &[
                CollectionListing {
                    entry_path: "games/Amiga/listings/games.txt",
                    genre: "AmigaVision",
                },
                CollectionListing {
                    entry_path: "games/Amiga/listings/demos.txt",
                    genre: "AmigaVision demos",
                },
            ],
            provenance: RuleProvenance::magik(
                "Explicit MagiK profile for AmigaVision archives: listings/games.txt and demos.txt enumerate launchable titles",
            ),
        }],
        ignore_rules: vec![IgnoreRule {
            file_names: &[
                "kick.rom",
                "kick13.rom",
                "kick20.rom",
                "kick31.rom",
                "kickstart.rom",
                "hrtmon.rom",
            ],
            extensions: &[],
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
        extensions: &["neo"],
        mount: MountSpec::load_file(1),
        disposition: PayloadDisposition::Playable,
        provenance: RuleProvenance::mgl(
            "Installed NeoGeo organizer MGLs use type=f index=1 for .neo payloads inside ZIPs",
        ),
    };

    LaunchProfile {
        id: "neogeo",
        system_id: "neogeo",
        category: "Arcade",
        title: "NeoGeo",
        core_name: "NeoGeo",
        core_path: Some("_Console/NeoGeo"),
        game_dirs: vec!["NEOGEO", "NeoGeo"],
        payload_rules: vec![
            PayloadRule {
                extensions: &["zip"],
                mount: MountSpec::load_file(1),
                disposition: PayloadDisposition::Playable,
                provenance: RuleProvenance::main(
                    "menu.cpp enables SCANO_NEOGEO; file_io.cpp treats .zip sets as selectable NeoGeo entries",
                ),
            },
            neo_rule,
        ],
        archive_entry_rules: vec![neo_rule],
        collection_rules: Vec::new(),
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::main(
            "Main routes NeoGeo file selection through SCANO_NEOGEO and neogeo_romset_tx",
        ),
    }
}

struct CartridgeProfileSpec {
    id: &'static str,
    system_id: &'static str,
    category: &'static str,
    title: &'static str,
    core_name: &'static str,
    core_path: &'static str,
    game_dirs: &'static [&'static str],
    extensions: &'static [&'static str],
}

fn cartridge_profile(spec: CartridgeProfileSpec) -> LaunchProfile {
    LaunchProfile {
        id: spec.id,
        system_id: spec.system_id,
        category: spec.category,
        title: spec.title,
        core_name: spec.core_name,
        core_path: Some(spec.core_path),
        game_dirs: spec.game_dirs.to_vec(),
        payload_rules: vec![PayloadRule {
            extensions: spec.extensions,
            mount: MountSpec::load_file(1),
            disposition: PayloadDisposition::Playable,
            provenance: RuleProvenance::mgl(
                "Existing organizer MGLs use type=f index=1 for cartridge payloads",
            ),
        }],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
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
