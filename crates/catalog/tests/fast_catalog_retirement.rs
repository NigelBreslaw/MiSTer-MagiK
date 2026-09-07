// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavior formerly tested through the retired whole-scan/SQLite harness.
#![cfg(feature = "builder")]
use mister_magik_catalog::fast_catalog_sources::build_independent_fast_snapshot;
use mister_magik_catalog::fast_five_catalog::FastFiveSnapshot;
use mister_magik_catalog::system_shard::SystemGame;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Corpus(PathBuf);
impl Corpus {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "magik-current-catalog-regressions-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        let corpus = Self(fs::canonicalize(path).unwrap());
        // Keep the complete snapshot nonempty while testing absent target systems.
        corpus.write("_Console/NES_20260828.rbf", b"core");
        corpus.write("games/NES/Anchor.nes", b"rom");
        corpus
    }
    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn scan(&self) -> FastFiveSnapshot {
        build_independent_fast_snapshot(&self.0).unwrap().0
    }
}
impl Drop for Corpus {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn games<'a>(snapshot: &'a FastFiveSnapshot, id: &str) -> &'a [SystemGame] {
    snapshot
        .systems
        .iter()
        .find(|system| system.system_id == id)
        .map_or(&[], |system| system.games.as_slice())
}

// Stored ZIP fixture with explicit little-endian headers; no installed media used.
fn stored_zip(names: &[&str]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut central = Vec::new();
    for name in names {
        let offset = bytes.len() as u32;
        bytes.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        bytes.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        bytes.extend_from_slice(&[0; 12]);
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&[0; 2]);
        bytes.extend_from_slice(name.as_bytes());
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0]);
        central.extend_from_slice(&[0; 20]);
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let offset = bytes.len() as u32;
    let size = central.len() as u32;
    bytes.extend(central);
    bytes.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&(names.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&(names.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&offset.to_le_bytes());
    bytes.extend_from_slice(&[0; 2]);
    bytes
}

#[test]
fn cartridge_archives_keep_visible_members_and_ignore_hidden_members() {
    let corpus = Corpus::new();
    corpus.write("_Console/SMS_20260828.rbf", b"core");
    corpus.write("games/SMS/Loose.sms", b"rom");
    corpus.write(
        "games/SMS/Packed.zip",
        &stored_zip(&[
            "Nested/Packed.sms",
            ".metadata/Hidden.sms",
            "._Resource.sms",
        ]),
    );
    let snapshot = corpus.scan();
    let rows = games(&snapshot, "sms");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|game| game.title == "Loose"));
    assert!(rows.iter().any(|game| game.title == "Packed"));
    assert!(rows.iter().all(|game| game.launch_plan.is_some()));
}

#[test]
fn nested_dynamic_games_remain_visible_but_hidden_and_unmatched_roots_do_not() {
    let corpus = Corpus::new();
    corpus.write("_Console/MyBeta_20260828.rbf", b"core");
    for relative in [
        "games/MyBeta/Publisher/First.rom",
        "games/MyBeta/Nested/Second.rom",
        "games/MyBeta/.metadata/Hidden.rom",
        "games/MyBeta/._Resource.rom",
        "games/Unmatched/Invisible.rom",
    ] {
        corpus.write(relative, b"rom");
    }
    let snapshot = corpus.scan();
    let rows = games(&snapshot, "mybeta");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|game| game.launch_plan.is_some()));
    assert!(rows.iter().any(|game| game.title == "First"));
    assert!(rows.iter().any(|game| game.title == "Second"));
    assert!(games(&snapshot, "unmatched").is_empty());
}

#[test]
fn shared_core_and_alias_roots_keep_distinct_systems_and_disc_launchability() {
    let corpus = Corpus::new();
    for core in ["Atari7800", "Saturn", "ZX-Spectrum"] {
        corpus.write(&format!("_Console/{core}_20260828.rbf"), b"core");
    }
    corpus.write("_Console/Atari 2600.mgl", br#"<mistergamedescription><rbf>_Console/Atari7800</rbf><setname>Atari2600</setname></mistergamedescription>"#);
    for path in [
        "games/Atari2600/First.a26",
        "games/Atari7800/Second.a78",
        "games/Spectrum/Tape.tzx",
        "games/Saturn/Disc.chd",
        "games/Saturn/Track.bin",
    ] {
        corpus.write(path, b"rom");
    }
    let snapshot = corpus.scan();
    for id in ["atari2600", "atari7800", "zx-spectrum", "saturn"] {
        let rows = games(&snapshot, id);
        assert_eq!(rows.len(), 1, "{id}");
        assert!(rows[0].launch_plan.is_some(), "{id}");
    }
    assert!(!games(&snapshot, "saturn")[0].launch_ref.ends_with(".bin"));
}

#[cfg(unix)]
#[test]
fn generic_discovery_does_not_follow_symlinked_game_directories_or_files() {
    let corpus = Corpus::new();
    corpus.write("_Console/SNES_20260828.rbf", b"core");
    corpus.write("games/SNES/Visible.sfc", b"rom");
    corpus.write("outside/Invisible.sfc", b"rom");
    std::os::unix::fs::symlink(corpus.0.join("outside"), corpus.0.join("games/SNES/Alias"))
        .unwrap();
    std::os::unix::fs::symlink(
        corpus.0.join("outside/Invisible.sfc"),
        corpus.0.join("games/SNES/Alias.sfc"),
    )
    .unwrap();
    let snapshot = corpus.scan();
    assert_eq!(games(&snapshot, "snes").len(), 1);
    assert_eq!(games(&snapshot, "snes")[0].title, "Visible");
}

#[test]
fn arcade_requires_rom_and_core_but_unknown_metadata_does_not_hide_playable_mras() {
    let corpus = Corpus::new();
    corpus.write("_Arcade/cores/TestCore_20260828.rbf", b"core");
    corpus.write("_Arcade/Unknown.mra", br#"<misterromdescription><name>Unknown</name><rbf>TestCore</rbf><rom zip="unknown.zip"><part>00</part></rom></misterromdescription>"#);
    corpus.write("games/mame/orphan.zip", b"not a launcher");
    assert!(games(&corpus.scan(), "arcade").is_empty());
    corpus.write("games/mame/unknown.zip", b"rom");
    let snapshot = corpus.scan();
    assert_eq!(games(&snapshot, "arcade").len(), 1);
    assert_eq!(games(&snapshot, "arcade")[0].title, "Unknown");
    assert!(
        games(&snapshot, "arcade")[0]
            .launch_ref
            .ends_with("Unknown.mra")
    );
    fs::remove_file(corpus.0.join("_Arcade/cores/TestCore_20260828.rbf")).unwrap();
    assert!(games(&corpus.scan(), "arcade").is_empty());
}

#[test]
fn incomplete_amigavision_does_not_publish_games_then_valid_listing_preserves_launch_identity() {
    let corpus = Corpus::new();
    corpus.write("games/Amiga/listings/games.txt", b"Agony & Pain\n");
    assert!(games(&corpus.scan(), "amiga").is_empty());
    corpus.write("games/Amiga/AmigaVision.hdf", b"hdf");
    let snapshot = corpus.scan();
    let game = games(&snapshot, "amiga")
        .iter()
        .find(|game| game.title == "Agony & Pain")
        .unwrap();
    assert_eq!(
        game.launch_ref,
        "magik-amigavision:games:Agony%20%26%20Pain"
    );
    assert!(!game.preview_asset_key.is_empty());
}
