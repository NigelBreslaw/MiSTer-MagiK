// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Filesystem artwork is read only for face-cache misses during preparation.
//! Packaging verifies hashes; runtime checks format/length. Six built-in roots
//! preserve the normal launcher when a binary-only Dev install lacks the pack.
#[cfg(any(feature = "ui", feature = "ui-preview", test))]
use crate::launcher_home::CardLevelSnapshot;
use mister_magik_catalog::bounded_file;
use mister_magik_framebuffer_scenes::launcher::LauncherArtwork;
#[cfg(any(feature = "ui", feature = "ui-preview", test))]
use mister_magik_framebuffer_scenes::launcher::{
    LauncherFaceCache, LauncherScene, LauncherTypography, PreparedLauncher,
};
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const RELATIVE_PATH: &str = "assets/ui/launcher-cards";
pub const SOURCE_BYTES: usize = 360 * 504 * 3;
const MAX_INDEX_BYTES: u64 = 128 * 1024;

#[derive(Deserialize)]
struct Index {
    schema: u32,
    width: u32,
    height: u32,
    format: String,
    cards: BTreeMap<String, Source>,
}
#[derive(Deserialize)]
struct Source {
    file: String,
    #[serde(default)]
    contains_name: bool,
}

pub fn asset_root() -> PathBuf {
    if let Some(path) = std::env::var_os("MISTER_MAGIK_CARD_ASSETS") {
        return PathBuf::from(path);
    }
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    {
        mister_magik_catalog::device_layout::current_app_path(RELATIVE_PATH)
    }
    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
    {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(RELATIVE_PATH)
    }
}

fn built_in(key: &str) -> Option<&'static [u8]> {
    Some(match key {
        "root:arcade" => include_bytes!("../assets/ui/launcher-cards/01_arcade.rgb888"),
        "root:consoles" => include_bytes!("../assets/ui/launcher-cards/02_consoles.rgb888"),
        "root:computers" => include_bytes!("../assets/ui/launcher-cards/03_computers.rgb888"),
        "root:handhelds" => include_bytes!("../assets/ui/launcher-cards/04_handhelds.rgb888"),
        "root:favourites" => include_bytes!("../assets/ui/launcher-cards/05_favourites.rgb888"),
        "root:settings" => include_bytes!("../assets/ui/launcher-cards/06_settings.rgb888"),
        _ => return None,
    })
}
fn index(root: &Path) -> std::io::Result<Index> {
    let bytes = bounded_file::read(root.join("index.json"), MAX_INDEX_BYTES)?;
    let index: Index = serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if index.schema != 1 || index.width != 360 || index.height != 504 || index.format != "RGB888" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsupported card artwork index",
        ));
    }
    Ok(index)
}
fn source(root: &Path, source: &Source) -> std::io::Result<Vec<u8>> {
    if !source.file.ends_with(".rgb888")
        || source.file.starts_with('.')
        || !source
            .file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid card artwork filename",
        ));
    }
    let bytes = bounded_file::read(root.join(&source.file), SOURCE_BYTES as u64)?;
    if bytes.len() != SOURCE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "card artwork length mismatch",
        ));
    }
    Ok(bytes)
}
fn retryable(error: &std::io::Error) -> bool {
    !matches!(
        error.kind(),
        std::io::ErrorKind::InvalidData
            | std::io::ErrorKind::InvalidInput
            | std::io::ErrorKind::PermissionDenied
    )
}
struct Loader<'a> {
    root: &'a Path,
    index: Option<std::io::Result<Index>>,
}
impl<'a> Loader<'a> {
    fn new(root: &'a Path) -> Self {
        Self { root, index: None }
    }
    fn load(&mut self, key: &str) -> LauncherArtwork {
        let normalized = mister_magik_catalog::catalog_classify::normalize_system_id(key);
        let builtin = built_in(&normalized);
        let fallback = |retry| LauncherArtwork {
            pixels: Cow::Borrowed(builtin.unwrap_or(&[])),
            retry,
            contains_name: false,
        };
        let index = self.index.get_or_insert_with(|| {
            let result = index(self.root);
            if let Err(error) = &result {
                eprintln!(
                    "card artwork {}: {error}; using fallback artwork",
                    self.root.display()
                );
            }
            result
        });
        let index = match index {
            Ok(index) => index,
            Err(error) => {
                // Absent/invalid packs and canonical built-in roots are stable.
                return fallback(
                    builtin.is_none()
                        && error.kind() != std::io::ErrorKind::NotFound
                        && retryable(error),
                );
            }
        };
        let Some(entry) = index
            .cards
            .get(key)
            .or_else(|| index.cards.get(&normalized))
        else {
            return fallback(false);
        };
        match source(self.root, entry) {
            Ok(bytes) => LauncherArtwork {
                pixels: Cow::Owned(bytes),
                retry: false,
                contains_name: entry.contains_name,
            },
            Err(error) => {
                eprintln!("card artwork {key}: {error}; using fallback artwork");
                fallback(builtin.is_none() && retryable(&error))
            }
        }
    }
}

/// Review/export convenience. Runtime uses the lazy callback below so it never
/// retains a level's full-resolution source images alongside prepared faces.
pub fn load_artwork(root: &Path, keys: &[String]) -> Vec<LauncherArtwork> {
    let mut loader = Loader::new(root);
    keys.iter().map(|key| loader.load(key)).collect()
}

pub fn load_cards(root: &Path, keys: &[String]) -> Vec<Cow<'static, [u8]>> {
    load_artwork(root, keys)
        .into_iter()
        .map(|source| source.pixels)
        .collect()
}

#[cfg(any(feature = "ui", feature = "ui-preview", test))]
#[derive(Default)]
pub(crate) struct CardFaceCache {
    faces: LauncherFaceCache,
    keys: Vec<String>,
    generation: u64,
    fallbacks: BTreeMap<String, bool>,
}
#[cfg(any(feature = "ui", feature = "ui-preview", test))]
impl CardFaceCache {
    pub fn retry_failed_artwork(&mut self) {
        self.fallbacks.retain(|_, retry| !*retry);
        self.faces.retry_failed_artwork();
    }
    pub fn prepare(
        &mut self,
        scene: LauncherScene,
        level: &CardLevelSnapshot,
        selected: usize,
        clock: &str,
        typography: Option<LauncherTypography<'_>>,
    ) -> PreparedLauncher {
        self.prepare_from(&asset_root(), scene, level, selected, clock, typography)
    }
    fn prepare_from(
        &mut self,
        root: &Path,
        scene: LauncherScene,
        level: &CardLevelSnapshot,
        selected: usize,
        clock: &str,
        typography: Option<LauncherTypography<'_>>,
    ) -> PreparedLauncher {
        if !self
            .keys
            .iter()
            .map(String::as_str)
            .eq(level.cards.iter().map(|c| c.artwork_key.as_str()))
        {
            self.fallbacks.clear();
            self.keys = level.cards.iter().map(|c| c.artwork_key.clone()).collect();
            self.generation = self.generation.wrapping_add(1).max(1);
        }
        let mut loader = Loader::new(root);
        level.with_data(selected, clock, |data| {
            scene
                .prepare_initial_with_rgb888_loader_and_cache(
                    data,
                    &mut |i| {
                        let key = &self.keys[i];
                        if let Some(&retry) = self.fallbacks.get(key) {
                            let normalized =
                                mister_magik_catalog::catalog_classify::normalize_system_id(key);
                            return LauncherArtwork {
                                pixels: Cow::Borrowed(built_in(&normalized).unwrap_or(&[])),
                                retry,
                                contains_name: false,
                            };
                        }
                        let result = loader.load(key);
                        if matches!(result.pixels, Cow::Borrowed(_)) {
                            self.fallbacks.insert(key.clone(), result.retry);
                        }
                        result
                    },
                    typography,
                    &mut self.faces,
                    self.generation,
                )
                .finish()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher_home::{CardLevelSnapshot, LauncherHomeCounts, LauncherHomeSnapshot};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "magik-card-assets-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn index(&self) {
            // Digests are intentionally irrelevant to the runtime read path.
            let manifest = serde_json::json!({"schema":1,"width":360,"height":504,"format":"RGB888","cards":{
                "snes":{"file":"snes.rgb888","sha256":"not-checked-at-runtime"},
                "bad":{"file":"snes.rgb888","sha256":"incorrect"},
                "n64":{"file":"n64.rgb888"},"escape":{"file":"../snes.rgb888"}
            }});
            std::fs::write(self.0.join("index.json"), manifest.to_string()).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn level() -> CardLevelSnapshot {
        let mut level = CardLevelSnapshot::root(&LauncherHomeSnapshot::from_counts(
            LauncherHomeCounts::default(),
        ));
        level.cards.truncate(2);
        level.cards[0].artwork_key = "snes".into();
        level.cards[1].artwork_key = "n64".into();
        level
    }
    #[test]
    fn binary_only_install_uses_the_same_six_root_images_without_heap_copies() {
        let f = Fixture::new();
        let level = CardLevelSnapshot::root(&LauncherHomeSnapshot::from_counts(
            LauncherHomeCounts::default(),
        ));
        let keys: Vec<_> = level.cards.iter().map(|c| c.artwork_key.clone()).collect();
        let installed = load_cards(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join(RELATIVE_PATH),
            &keys,
        );
        let fallback = load_cards(&f.0, &keys);
        assert_eq!(installed, fallback);
        assert!(
            fallback
                .iter()
                .all(|p| matches!(p, Cow::Borrowed(_)) && p.len() == SOURCE_BYTES)
        );
        let mut cache = CardFaceCache::default();
        let prepared =
            cache.prepare_from(&f.0, LauncherScene::new(960, 540), &level, 0, "12:00", None);
        assert!(!prepared.needs_artwork_retry());
        // Exercise the default path too, without changing global environment.
        cache.prepare(LauncherScene::new(960, 540), &level, 0, "12:01", None);
    }
    #[test]
    fn declared_root_source_failure_uses_builtin_and_recovers_after_repair() {
        let f = Fixture::new();
        let manifest = serde_json::json!({"schema":1,"width":360,"height":504,"format":"RGB888","cards":{
            "root:arcade":{"file":"repaired.rgb888"}
        }});
        std::fs::write(f.0.join("index.json"), manifest.to_string()).unwrap();
        let mut loader = Loader::new(&f.0);
        let fallback = loader.load("root:arcade");
        assert!(!fallback.retry);
        assert_eq!(fallback.pixels.as_ref(), built_in("root:arcade").unwrap());
        std::fs::write(f.0.join("repaired.rgb888"), vec![80; SOURCE_BYTES]).unwrap();
        let recovered = Loader::new(&f.0).load("root:arcade");
        assert!(!recovered.retry);
        assert_eq!(recovered.pixels.as_ref(), vec![80; SOURCE_BYTES]);
    }

    #[test]
    fn wordmark_policy_only_applies_to_valid_installed_pixels() {
        let f = Fixture::new();
        let manifest = serde_json::json!({"schema":1,"width":360,"height":504,"format":"RGB888","cards":{
            "menu:consoles:nintendo":{"file":"logo.rgb888","contains_name":true}
        }});
        std::fs::write(f.0.join("index.json"), manifest.to_string()).unwrap();
        assert!(
            !Loader::new(&f.0)
                .load("menu:consoles:nintendo")
                .contains_name
        );
        std::fs::write(f.0.join("logo.rgb888"), vec![123; SOURCE_BYTES]).unwrap();
        assert!(
            Loader::new(&f.0)
                .load("menu:consoles:nintendo")
                .contains_name
        );
        std::fs::write(f.0.join("logo.rgb888"), b"truncated").unwrap();
        assert!(
            !Loader::new(&f.0)
                .load("menu:consoles:nintendo")
                .contains_name
        );
    }

    #[test]
    fn filesystem_cards_preserve_slots_and_fail_independently() {
        let f = Fixture::new();
        f.index();
        let bytes = vec![123; SOURCE_BYTES];
        std::fs::write(f.0.join("snes.rgb888"), &bytes).unwrap();
        let keys = ["missing", "snes", "bad", "escape", " SNES "].map(String::from);
        let loaded = load_cards(&f.0, &keys);
        assert!(loaded[0].is_empty() && loaded[3].is_empty());
        assert_eq!(loaded[1].as_ref(), bytes);
        assert_eq!(loaded[2].as_ref(), bytes);
        assert_eq!(loaded[4].as_ref(), bytes);
        for bad in [b"truncated".to_vec(), vec![0; SOURCE_BYTES + 1]] {
            std::fs::write(f.0.join("snes.rgb888"), bad).unwrap();
            assert!(load_cards(&f.0, &keys).iter().all(|p| p.is_empty()));
        }
    }
    #[test]
    fn failed_slots_retry_successful_faces_reuse_and_key_changes_advance_generation() {
        let f = Fixture::new();
        f.index();
        std::fs::write(f.0.join("snes.rgb888"), vec![123; SOURCE_BYTES]).unwrap();
        let mut level = level();
        let scene = LauncherScene::new(960, 540);
        let mut cache = CardFaceCache::default();
        let first = cache.prepare_from(&f.0, scene, &level, 0, "12:00", None);
        assert!(first.needs_artwork_retry());
        let generation = cache.generation;
        // A successful source is no longer available: an unchanged face must not read it.
        std::fs::remove_file(f.0.join("snes.rgb888")).unwrap();
        std::fs::write(f.0.join("n64.rgb888"), vec![231; SOURCE_BYTES]).unwrap();
        cache.retry_failed_artwork();
        let recovered = cache.prepare_from(&f.0, scene, &level, 0, "12:01", None);
        assert!(!recovered.needs_artwork_retry());
        assert_eq!(cache.generation, generation);
        // A count change rebuilds only that card, exposing its removed source.
        level.cards[0].games = Some(42);
        let changed = cache.prepare_from(&f.0, scene, &level, 0, "12:01", None);
        assert!(changed.needs_artwork_retry());
        assert_eq!(cache.generation, generation);
        std::fs::write(f.0.join("snes.rgb888"), vec![123; SOURCE_BYTES]).unwrap();
        level.cards.swap(0, 1);
        cache.prepare_from(&f.0, scene, &level, 0, "12:01", None);
        assert_eq!(cache.generation, generation + 1);
    }
    #[test]
    fn count_refreshes_do_not_retry_failed_sources_before_explicit_retry() {
        let f = Fixture::new();
        f.index();
        let mut level = level();
        let scene = LauncherScene::new(960, 540);
        let mut cache = CardFaceCache::default();
        let first = cache.prepare_from(&f.0, scene, &level, 0, "12:00", None);
        assert!(first.needs_artwork_retry());
        for name in ["snes", "n64"] {
            std::fs::write(f.0.join(format!("{name}.rgb888")), vec![42; SOURCE_BYTES]).unwrap();
        }
        // Repair is deliberately invisible to ordinary count/label preparation.
        for count in 1..4 {
            level.cards[0].games = Some(count);
            let prepared = cache.prepare_from(&f.0, scene, &level, 0, "12:00", None);
            assert!(prepared.needs_artwork_retry());
        }
        cache.retry_failed_artwork();
        assert!(
            !cache
                .prepare_from(&f.0, scene, &level, 0, "12:00", None)
                .needs_artwork_retry()
        );
    }

    #[test]
    fn permanent_pack_and_source_errors_do_not_schedule_retries() {
        let f = Fixture::new();
        for manifest in [
            b"bad json".as_slice(),
            b"{\"schema\":2,\"width\":360,\"height\":504,\"format\":\"RGB888\",\"cards\":{}}"
                .as_slice(),
        ] {
            std::fs::write(f.0.join("index.json"), manifest).unwrap();
            for key in ["root:arcade", "snes"] {
                assert!(!Loader::new(&f.0).load(key).retry);
            }
        }
        std::fs::write(
            f.0.join("index.json"),
            vec![b' '; MAX_INDEX_BYTES as usize + 1],
        )
        .unwrap();
        assert!(!Loader::new(&f.0).load("snes").retry);
        std::fs::remove_file(f.0.join("index.json")).unwrap();
        std::fs::create_dir(f.0.join("index.json")).unwrap();
        assert!(!Loader::new(&f.0).load("snes").retry);
        std::fs::remove_dir(f.0.join("index.json")).unwrap();
        f.index();
        assert!(!Loader::new(&f.0).load("escape").retry);
        for bytes in [vec![0; 1], vec![0; SOURCE_BYTES + 1]] {
            std::fs::write(f.0.join("snes.rgb888"), bytes).unwrap();
            assert!(!Loader::new(&f.0).load("snes").retry);
        }
    }

    #[test]
    fn missing_index_is_stable_until_artwork_keys_change() {
        let f = Fixture::new();
        let mut level = level();
        let scene = LauncherScene::new(960, 540);
        let mut cache = CardFaceCache::default();
        let first = cache.prepare_from(&f.0, scene, &level, 0, "12:00", None);
        assert!(!first.needs_artwork_retry());
        f.index();
        for name in ["snes", "n64"] {
            std::fs::write(f.0.join(format!("{name}.rgb888")), vec![42; SOURCE_BYTES]).unwrap();
        }
        let unchanged = cache.prepare_from(&f.0, scene, &level, 0, "12:00", None);
        assert!(!unchanged.needs_artwork_retry());
        assert!(first.shares_faces_with(&unchanged));
        level.cards.swap(0, 1);
        let changed = cache.prepare_from(&f.0, scene, &level, 0, "12:00", None);
        assert!(!changed.needs_artwork_retry());
        assert!(!first.shares_faces_with(&changed));
    }
}
