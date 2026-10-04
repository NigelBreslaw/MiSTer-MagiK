// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Installed card sources. Preparation owns all I/O; the frame loop only sees
//! cached surfaces. One RGB888 source feeds both HDMI and native CRT filtering.
#[cfg(any(feature = "ui", feature = "ui-preview", test))]
use crate::launcher_home::CardLevelSnapshot;
#[cfg(any(feature = "ui", feature = "ui-preview", test))]
use mister_magik_framebuffer_scenes::launcher::LauncherFaceCache;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const RELATIVE_PATH: &str = "assets/ui/launcher-cards";
pub const SOURCE_BYTES: usize = 360 * 504 * 3;
const MAX_INDEX_BYTES: usize = 128 * 1024;

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
    sha256: String,
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

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("oversized card asset".into());
    }
    Ok(bytes)
}

fn index(root: &Path) -> Result<Index, String> {
    let bytes = read_bounded(&root.join("index.json"), MAX_INDEX_BYTES)?;
    let index: Index = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if index.schema != 1 || index.width != 360 || index.height != 504 || index.format != "RGB888" {
        return Err("unsupported card artwork index".into());
    }
    Ok(index)
}

fn source(root: &Path, source: &Source) -> Result<Vec<u8>, String> {
    // Manifest filenames are flat. Never interpret a taxonomy ID as a path.
    if !source.file.ends_with(".rgb888")
        || source.file.starts_with('.')
        || !source
            .file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err("invalid card artwork filename".into());
    }
    let bytes = read_bounded(&root.join(&source.file), SOURCE_BYTES)?;
    if bytes.len() != SOURCE_BYTES
        || Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            != source.sha256
    {
        return Err("card artwork length or checksum mismatch".into());
    }
    Ok(bytes)
}

/// Empty entries deliberately retain their slot and use the renderer's generic
/// card face. A missing/corrupt optional asset must not prevent opening MagiK.
pub fn load_cards(root: &Path, keys: &[String]) -> Vec<Vec<u8>> {
    let index = match index(root) {
        Ok(index) => index,
        Err(error) => {
            eprintln!(
                "card artwork {}: {error}; using generic cards",
                root.display()
            );
            return vec![Vec::new(); keys.len()];
        }
    };
    keys.iter()
        .map(|key| {
            let normalized = mister_magik_catalog::catalog_classify::normalize_system_id(key);
            let Some(entry) = index
                .cards
                .get(key)
                .or_else(|| index.cards.get(&normalized))
            else {
                return Vec::new();
            };
            source(root, entry).unwrap_or_else(|error| {
                eprintln!("card artwork {key}: {error}; using generic card");
                Vec::new()
            })
        })
        .collect()
}

#[cfg(any(feature = "ui", feature = "ui-preview", test))]
#[derive(Default)]
pub(crate) struct CardFaceCache {
    pub faces: LauncherFaceCache,
    pub artwork: Vec<Vec<u8>>,
    keys: Option<Vec<String>>,
}
#[cfg(any(feature = "ui", feature = "ui-preview", test))]
impl CardFaceCache {
    pub fn load(&mut self, level: &CardLevelSnapshot) {
        self.load_from(&asset_root(), level);
    }

    fn load_from(&mut self, root: &Path, level: &CardLevelSnapshot) {
        if self.keys.as_ref().is_some_and(|keys| {
            keys.iter()
                .map(String::as_str)
                .eq(level.cards.iter().map(|card| card.artwork_key.as_str()))
        }) {
            return;
        }
        let keys: Vec<_> = level
            .cards
            .iter()
            .map(|card| card.artwork_key.clone())
            .collect();
        self.artwork = load_cards(root, &keys);
        self.faces = LauncherFaceCache::default();
        self.keys = Some(keys);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn filesystem_cards_preserve_slots_and_fail_independently() {
        let f = Fixture::new();
        let bytes = vec![123; SOURCE_BYTES];
        std::fs::write(f.0.join("console.rgb888"), &bytes).unwrap();
        let entry = serde_json::json!({"file":"console.rgb888", "sha256":Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>()});
        let manifest = serde_json::json!({"schema":1,"width":360,"height":504,"format":"RGB888", "cards":{
            "snes":entry, "bad": {"file":"console.rgb888", "sha256":"incorrect"},
            "escape":{"file":"../console.rgb888", "sha256":entry["sha256"]}
        }});
        std::fs::write(f.0.join("index.json"), manifest.to_string()).unwrap();
        let keys = ["missing", "snes", "bad", "escape", " SNES "].map(String::from);
        let loaded = load_cards(&f.0, &keys);
        assert_eq!(loaded, vec![vec![], bytes.clone(), vec![], vec![], bytes]);
        std::fs::write(f.0.join("console.rgb888"), b"truncated").unwrap();
        assert!(load_cards(&f.0, &keys).iter().all(Vec::is_empty));
        std::fs::write(f.0.join("console.rgb888"), vec![0; SOURCE_BYTES + 1]).unwrap();
        assert!(load_cards(&f.0, &keys).iter().all(Vec::is_empty));
    }

    #[test]
    fn cached_sources_survive_refresh_without_file_io_and_follow_identity_changes() {
        use crate::launcher_home::{LauncherHomeCounts, LauncherHomeSnapshot};
        let mut level = CardLevelSnapshot::root(&LauncherHomeSnapshot::from_counts(
            LauncherHomeCounts::default(),
        ));
        let mut cache = CardFaceCache::default();
        cache.load(&level);
        assert!(
            cache
                .artwork
                .iter()
                .all(|pixels| pixels.len() == SOURCE_BYTES)
        );
        let saved = cache.artwork.clone();
        let missing = Fixture::new();
        level.cards[0].games = Some(42);
        level.cards[0].name = "renamed label".into();
        cache.load_from(&missing.0, &level);
        assert_eq!(
            cache.artwork, saved,
            "count/label refresh must not read files"
        );
        level.cards.swap(0, 1);
        cache.load_from(&missing.0, &level);
        assert!(
            cache.artwork.iter().all(Vec::is_empty),
            "new identities invalidate artwork and faces"
        );
    }

    #[test]
    fn missing_or_incompatible_index_falls_back_without_losing_cards() {
        let f = Fixture::new();
        let keys = ["snes".into(), "n64".into()];
        for bytes in [
            b"not json".as_slice(),
            br#"{"schema":2,"width":360,"height":504,"format":"RGB888","cards":{}}"#,
        ] {
            std::fs::write(f.0.join("index.json"), bytes).unwrap();
            assert_eq!(load_cards(&f.0, &keys), vec![Vec::<u8>::new(); 2]);
        }
        std::fs::remove_file(f.0.join("index.json")).unwrap();
        assert_eq!(load_cards(&f.0, &keys), vec![Vec::<u8>::new(); 2]);
    }
}
