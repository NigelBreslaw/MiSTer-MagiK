// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Static facts shown on a system's page: maker, release year and generation.
//! Keyed by catalog system ID (see the taxonomy); unknown systems simply show
//! less rather than something invented.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SystemFacts {
    pub maker: &'static str,
    pub year: u16,
    pub generation: &'static str,
}

const fn facts(maker: &'static str, year: u16, generation: &'static str) -> Option<SystemFacts> {
    Some(SystemFacts {
        maker,
        year,
        generation,
    })
}

pub fn system_facts(system_id: &str) -> Option<SystemFacts> {
    match system_id {
        // Consoles.
        "atari2600" => facts("ATARI", 1977, "8-BIT"),
        "atari5200" => facts("ATARI", 1982, "8-BIT"),
        "atari7800" => facts("ATARI", 1986, "8-BIT"),
        "jaguar" => facts("ATARI", 1993, "64-BIT"),
        "sg1000" => facts("SEGA", 1983, "8-BIT"),
        "sms" => facts("SEGA", 1985, "8-BIT"),
        "megadrive" => facts("SEGA", 1988, "16-BIT"),
        "megacd" => facts("SEGA", 1991, "16-BIT"),
        "s32x" => facts("SEGA", 1994, "32-BIT"),
        "saturn" => facts("SEGA", 1994, "32-BIT"),
        "psx" => facts("SONY", 1994, "32-BIT"),
        "nes" => facts("NINTENDO", 1983, "8-BIT"),
        "fds" => facts("NINTENDO", 1986, "8-BIT"),
        "snes" => facts("NINTENDO", 1990, "16-BIT"),
        "satellaview" => facts("NINTENDO", 1995, "16-BIT"),
        "n64" => facts("NINTENDO", 1996, "64-BIT"),
        "tgfx16" => facts("NEC", 1987, "8-BIT"),
        "tgfx16-cd" => facts("NEC", 1988, "8-BIT"),
        "supergrafx" => facts("NEC", 1989, "8-BIT"),
        "neogeo" | "neo-geo" | "snk-neo-geo" => facts("SNK", 1990, "16-BIT"),
        "neogeo-cd" => facts("SNK", 1994, "16-BIT"),
        "channelf" => facts("FAIRCHILD", 1976, "8-BIT"),
        "astrocade" => facts("BALLY", 1977, "8-BIT"),
        "odyssey2" => facts("MAGNAVOX", 1978, "8-BIT"),
        "intellivision" => facts("MATTEL", 1979, "16-BIT"),
        "colecovision" | "coleco" => facts("COLECO", 1982, "8-BIT"),
        "vectrex" => facts("GCE", 1982, "8-BIT"),
        "arcadia" => facts("EMERSON", 1982, "8-BIT"),
        "creativision" => facts("VTECH", 1981, "8-BIT"),
        "casio-pv-1000" => facts("CASIO", 1983, "8-BIT"),
        // Handhelds.
        "gb" | "gameboy" | "gameboy2p" | "gameboy-sinden" => facts("NINTENDO", 1989, "8-BIT"),
        "gbc" => facts("NINTENDO", 1998, "8-BIT"),
        "gba" | "gba2p" => facts("NINTENDO", 2001, "32-BIT"),
        "sgb" | "sgb2" => facts("NINTENDO", 1994, "16-BIT"),
        "pokemonmini" => facts("NINTENDO", 2001, "8-BIT"),
        "gamegear" => facts("SEGA", 1990, "8-BIT"),
        "atarilynx" => facts("ATARI", 1989, "16-BIT"),
        "neogeopocket" => facts("SNK", 1998, "16-BIT"),
        "ngpc" => facts("SNK", 1999, "16-BIT"),
        "wonderswan" => facts("BANDAI", 1999, "16-BIT"),
        "wonderswancolor" => facts("BANDAI", 2000, "16-BIT"),
        "supervision" => facts("WATARA", 1992, "8-BIT"),
        "megaduck" => facts("CREATRONIC", 1993, "8-BIT"),
        // Computers.
        "acornatom" => facts("ACORN", 1979, "8-BIT"),
        "acornelectron" => facts("ACORN", 1983, "8-BIT"),
        "bbcmicro" => facts("ACORN", 1981, "8-BIT"),
        "archie" => facts("ACORN", 1987, "32-BIT"),
        "apple-ii" => facts("APPLE", 1977, "8-BIT"),
        "macplus" => facts("APPLE", 1986, "16-BIT"),
        "maclc" => facts("APPLE", 1990, "32-BIT"),
        "amiga" => facts("COMMODORE", 1985, "32-BIT"),
        "amigacd32" => facts("COMMODORE", 1993, "32-BIT"),
        "c64" => facts("COMMODORE", 1982, "8-BIT"),
        "c128" => facts("COMMODORE", 1985, "8-BIT"),
        "c16" => facts("COMMODORE", 1984, "8-BIT"),
        "vic20" => facts("COMMODORE", 1980, "8-BIT"),
        "pet2001" => facts("COMMODORE", 1977, "8-BIT"),
        "atari800" => facts("ATARI", 1979, "8-BIT"),
        "atarist" => facts("ATARI", 1985, "16-BIT"),
        "zx81" => facts("SINCLAIR", 1981, "8-BIT"),
        "zx-spectrum" | "spectrum" => facts("SINCLAIR", 1982, "8-BIT"),
        "ql" => facts("SINCLAIR", 1984, "16-BIT"),
        "trs-80" => facts("TANDY", 1977, "8-BIT"),
        "coco2" => facts("TANDY", 1980, "8-BIT"),
        "coco3" => facts("TANDY", 1986, "8-BIT"),
        "msx" | "msx1" => facts("MSX", 1983, "8-BIT"),
        "msx2" => facts("MSX", 1985, "8-BIT"),
        "pc88" => facts("NEC", 1981, "8-BIT"),
        "pc98" => facts("NEC", 1982, "16-BIT"),
        "x68000" => facts("SHARP", 1987, "16-BIT"),
        "x1" | "sharp-x1" => facts("SHARP", 1982, "8-BIT"),
        "fm7" => facts("FUJITSU", 1982, "8-BIT"),
        "fmtowns" => facts("FUJITSU", 1989, "32-BIT"),
        "altair8800" => facts("MITS", 1975, "8-BIT"),
        "ti-99-4a" => facts("TI", 1981, "16-BIT"),
        "dos" => facts("MICROSOFT", 1981, "16-BIT"),
        "eg2000" => facts("EACA", 1982, "8-BIT"),
        "oric" => facts("TANGERINE", 1983, "8-BIT"),
        "aquarius" => facts("MATTEL", 1983, "8-BIT"),
        "casio-pv-2000" => facts("CASIO", 1983, "8-BIT"),
        "amstrad" => facts("AMSTRAD", 1984, "8-BIT"),
        "samcoupe" => facts("MGT", 1989, "8-BIT"),
        "ao486" => facts("PC", 1989, "32-BIT"),
        _ => None,
    }
}

/// `NINTENDO / 1990 / 16-BIT`, or whatever part is known; empty if none is.
pub fn system_subtitle(system_id: &str) -> String {
    system_facts(system_id).map_or_else(String::new, |facts| {
        format!("{} / {} / {}", facts.maker, facts.year, facts.generation)
    })
}

/// What the focused tile of a system's page opens; `section` is 0 Games,
/// 1 Recent, 2 Favourites.
pub fn hub_caption(section: usize, games: usize, recent: usize, favourites: usize) -> String {
    match section {
        0 => format!("BROWSE ALL {games} GAMES"),
        1 if recent == 0 => "NOTHING PLAYED YET".into(),
        1 => "PICK UP WHERE YOU LEFT OFF".into(),
        _ if favourites == 0 => "NO FAVOURITES YET".into(),
        _ => format!(
            "{favourites} SAVED FAVOURITE{}",
            if favourites == 1 { "" } else { "S" }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions_describe_what_the_tile_opens() {
        assert_eq!(hub_caption(0, 1729, 0, 0), "BROWSE ALL 1729 GAMES");
        assert_eq!(hub_caption(1, 9, 0, 0), "NOTHING PLAYED YET");
        assert_eq!(hub_caption(1, 9, 4, 0), "PICK UP WHERE YOU LEFT OFF");
        assert_eq!(hub_caption(2, 9, 0, 0), "NO FAVOURITES YET");
        assert_eq!(hub_caption(2, 9, 0, 1), "1 SAVED FAVOURITE");
        assert_eq!(hub_caption(2, 9, 0, 3), "3 SAVED FAVOURITES");
    }

    #[test]
    fn known_systems_have_a_subtitle_and_unknown_ones_stay_empty() {
        assert_eq!(system_subtitle("snes"), "NINTENDO / 1990 / 16-BIT");
        assert_eq!(system_subtitle("gamegear"), "SEGA / 1990 / 8-BIT");
        assert_eq!(system_subtitle("not-a-system"), "");
    }

    #[test]
    fn every_taxonomy_system_with_a_page_has_facts() {
        use mister_magik_catalog::catalog_classify::{LauncherSection, system_definitions};
        // Arcade boards use the cabinet and have no identity line.
        let has_page = |section: LauncherSection| {
            matches!(
                section,
                LauncherSection::Consoles
                    | LauncherSection::Handhelds
                    | LauncherSection::Computers
                    | LauncherSection::SnkNeogeo
            )
        };
        let definitions = system_definitions().expect("valid taxonomy");
        let mut missing = Vec::new();
        for definition in definitions.iter().filter(|d| has_page(d.section)) {
            for id in std::iter::once(&definition.id).chain(&definition.aliases) {
                if system_facts(id).is_none() {
                    missing.push(id.as_str());
                }
            }
        }
        assert!(missing.is_empty(), "systems without facts: {missing:?}");
    }
}
