// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded, non-destructive import of legacy MiSTer SNES user state.

use crate::user_state::{UnresolvedImport, UserGameIdentity, UserStateStore};
use std::fs;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

const IMPORT_SOURCE: &str = "legacy-snes";
const IMPORT_VERSION: u32 = 1;
const RECENT_RECORD_BYTES: usize = 1024 + 256 + 256;
const RECENT_RECORD_LIMIT: usize = 16;
const MAX_TEXT_FAVOURITES_BYTES: u64 = 256 * 1024;
const MAX_FAVOURITE_ENTRIES: usize = 4096;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LegacyImportReport {
    pub favourites_imported: usize,
    pub recents_imported: usize,
    pub unresolved: usize,
    pub already_imported: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyRecentRecord {
    pub path: String,
    pub title: String,
}

pub fn import_legacy_snes(
    store: &UserStateStore,
    games: &[UserGameIdentity],
    media_root: &Path,
    now: i64,
) -> Result<LegacyImportReport, String> {
    if store.imported_version(IMPORT_SOURCE)? == Some(IMPORT_VERSION) {
        return Ok(LegacyImportReport {
            already_imported: true,
            ..LegacyImportReport::default()
        });
    }

    let mut report = LegacyImportReport::default();
    let config = media_root.join("config");
    if let Ok(entries) = fs::read_dir(&config) {
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let Some(name) = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_ascii_lowercase)
            else {
                continue;
            };
            if name.contains("_recent_") && name.ends_with(".cfg") {
                import_recent_file(store, games, &path, &mut report)?;
            } else if name.contains("snes") && name.contains("favorite") && name.ends_with(".cfg") {
                import_favourite_file(store, games, &path, now, &mut report)?;
            }
        }
    }

    for name in ["_@Favorites", "_Favorites", "Favorites"] {
        import_favourite_tree(store, games, &media_root.join(name), now, &mut report)?;
    }
    store.mark_imported(IMPORT_SOURCE, IMPORT_VERSION, now)?;
    Ok(report)
}

pub fn parse_main_recent(data: &[u8]) -> Result<Vec<LegacyRecentRecord>, String> {
    if data.len() > RECENT_RECORD_BYTES * RECENT_RECORD_LIMIT {
        return Err("legacy recent file exceeds 16 records".to_string());
    }
    if !data.len().is_multiple_of(RECENT_RECORD_BYTES) {
        return Err("legacy recent file has a truncated record".to_string());
    }
    let mut records = Vec::new();
    for record in data.as_chunks::<RECENT_RECORD_BYTES>().0 {
        let dir = nul_terminated(&record[..1024], "directory")?;
        let name = nul_terminated(&record[1024..1280], "name")?;
        let title = nul_terminated(&record[1280..], "label")?;
        if name.is_empty() {
            break;
        }
        let path = if dir.is_empty() {
            name.to_string()
        } else {
            format!("{dir}/{name}")
        };
        records.push(LegacyRecentRecord {
            path: normalize_path(&path),
            title: if title.is_empty() { name } else { title }.to_string(),
        });
    }
    Ok(records)
}

fn import_recent_file(
    store: &UserStateStore,
    games: &[UserGameIdentity],
    path: &Path,
    report: &mut LegacyImportReport,
) -> Result<(), String> {
    let data = fs::read(path)
        .map_err(|error| format!("read legacy recent {}: {error}", path.display()))?;
    let records = parse_main_recent(&data)?;
    let anchor = modified_unix(path).unwrap_or(0);
    for (index, record) in records.iter().enumerate() {
        let played_at = anchor.saturating_sub(i64::try_from(index).unwrap_or(i64::MAX));
        if let Some(game) = match_game(games, &record.path, None) {
            store.record_play(game, played_at)?;
            report.recents_imported += 1;
        } else {
            unresolved(
                store,
                path,
                "recent",
                &record.path,
                &record.title,
                played_at,
                report,
            )?;
        }
    }
    Ok(())
}

fn import_favourite_file(
    store: &UserStateStore,
    games: &[UserGameIdentity],
    path: &Path,
    now: i64,
    report: &mut LegacyImportReport,
) -> Result<(), String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("inspect legacy favourites {}: {error}", path.display()))?;
    if metadata.len() > MAX_TEXT_FAVOURITES_BYTES {
        return Err(format!("legacy favourites {} is too large", path.display()));
    }
    let text = fs::read_to_string(path)
        .map_err(|error| format!("read legacy favourites {}: {error}", path.display()))?;
    for value in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        import_favourite_candidate(store, games, path, value, None, now, report)?;
    }
    Ok(())
}

fn import_favourite_tree(
    store: &UserStateStore,
    games: &[UserGameIdentity],
    root: &Path,
    now: i64,
    report: &mut LegacyImportReport,
) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    let mut seen = 0usize;
    for entry in WalkDir::new(root)
        .follow_links(false)
        .max_depth(8)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() && !entry.path_is_symlink() {
            continue;
        }
        seen += 1;
        if seen > MAX_FAVOURITE_ENTRIES {
            return Err(format!(
                "legacy favourites tree {} exceeds entry limit",
                root.display()
            ));
        }
        let path = entry.path();
        let target = if entry.path_is_symlink() {
            fs::read_link(path).ok().map(|target| {
                if target.is_absolute() {
                    target
                } else {
                    path.parent().unwrap_or(root).join(target)
                }
            })
        } else {
            None
        };
        let text = path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("mgl"))
            .then(|| fs::read_to_string(path).ok())
            .flatten();
        import_favourite_candidate(
            store,
            games,
            path,
            target.as_deref().unwrap_or(path).to_string_lossy().as_ref(),
            text.as_deref(),
            now,
            report,
        )?;
    }
    Ok(())
}

fn import_favourite_candidate(
    store: &UserStateStore,
    games: &[UserGameIdentity],
    source: &Path,
    candidate: &str,
    wrapper_text: Option<&str>,
    now: i64,
    report: &mut LegacyImportReport,
) -> Result<(), String> {
    if let Some(game) = match_game(games, candidate, wrapper_text) {
        store.set_favourite(game, true, now)?;
        report.favourites_imported += 1;
    } else {
        unresolved(
            store,
            source,
            "favourite",
            candidate,
            source
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("Favourite"),
            now,
            report,
        )?;
    }
    Ok(())
}

fn match_game<'a>(
    games: &'a [UserGameIdentity],
    candidate: &str,
    wrapper_text: Option<&str>,
) -> Option<&'a UserGameIdentity> {
    let candidate = normalize_path(candidate);
    games.iter().find(|game| {
        game.system_id.eq_ignore_ascii_case("snes")
            && ([game.launch_ref.as_str(), game.payload_path.as_str()]
                .into_iter()
                .any(|path| normalize_path(path) == candidate)
                || wrapper_text.is_some_and(|text| {
                    [game.launch_ref.as_str(), game.payload_path.as_str()]
                        .into_iter()
                        .filter(|path| !path.is_empty())
                        .any(|path| text.contains(path))
                }))
    })
}

fn unresolved(
    store: &UserStateStore,
    source: &Path,
    kind: &str,
    path: &str,
    title: &str,
    observed_at: i64,
    report: &mut LegacyImportReport,
) -> Result<(), String> {
    store.add_unresolved_import(&UnresolvedImport {
        source: source.display().to_string(),
        kind: kind.to_string(),
        path: path.to_string(),
        title: title.to_string(),
        observed_at,
    })?;
    report.unresolved += 1;
    Ok(())
}

fn nul_terminated<'a>(bytes: &'a [u8], field: &str) -> Result<&'a str, String> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).map_err(|error| format!("invalid {field} text: {error}"))
}

fn normalize_path(path: &str) -> String {
    let mut result = path.replace('\\', "/");
    while result.contains("//") {
        result = result.replace("//", "/");
    }
    result.trim_end_matches('/').to_string()
}

fn modified_unix(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mister-magik-legacy-import-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn game(key: &str) -> UserGameIdentity {
        UserGameIdentity {
            system_id: "snes".to_string(),
            stable_key: key.to_string(),
            title: key.to_string(),
            launch_ref: format!("/media/fat/games/SNES/{key}.sfc"),
            payload_path: format!("/media/fat/games/SNES/{key}.sfc"),
        }
    }

    fn record(dir: &str, name: &str, title: &str) -> Vec<u8> {
        let mut bytes = vec![0; RECENT_RECORD_BYTES];
        bytes[..dir.len()].copy_from_slice(dir.as_bytes());
        bytes[1024..1024 + name.len()].copy_from_slice(name.as_bytes());
        bytes[1280..1280 + title.len()].copy_from_slice(title.as_bytes());
        bytes
    }

    #[test]
    fn parses_main_records_and_rejects_truncation() {
        let bytes = record("/media/fat/games/SNES", "one.sfc", "One");
        assert_eq!(
            parse_main_recent(&bytes).unwrap(),
            vec![LegacyRecentRecord {
                path: "/media/fat/games/SNES/one.sfc".to_string(),
                title: "One".to_string(),
            }]
        );
        assert!(parse_main_recent(&bytes[..bytes.len() - 1]).is_err());
        assert!(
            parse_main_recent(&vec![0; RECENT_RECORD_BYTES * (RECENT_RECORD_LIMIT + 1)]).is_err()
        );
    }

    #[test]
    fn imports_recents_and_favourites_once() {
        let root = temp_root("complete");
        fs::create_dir_all(root.join("config")).unwrap();
        let mut recent = record("/media/fat/games/SNES", "one.sfc", "One");
        recent.extend(record("/missing", "lost.sfc", "Lost"));
        fs::write(root.join("config/SNES_recent_0.cfg"), recent).unwrap();
        fs::write(
            root.join("config/SNES_favorites.cfg"),
            "/media/fat/games/SNES/two.sfc\n/missing/favourite.sfc\n",
        )
        .unwrap();
        let store = UserStateStore::open(root.join("user-state.sqlite3")).unwrap();
        let games = vec![game("one"), game("two")];

        let report = import_legacy_snes(&store, &games, &root, 100).unwrap();
        assert_eq!(report.recents_imported, 1);
        assert_eq!(report.favourites_imported, 1);
        assert_eq!(report.unresolved, 2);
        assert_eq!(store.recent_unique("snes", 16).unwrap().len(), 1);
        assert_eq!(store.favourite_count("snes").unwrap(), 1);

        let repeated = import_legacy_snes(&store, &games, &root, 200).unwrap();
        assert!(repeated.already_imported);
        assert_eq!(store.recent_unique("snes", 16).unwrap()[0].play_count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn imports_conventional_favourite_symlink() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        fs::create_dir_all(root.join("_@Favorites/SNES")).unwrap();
        symlink(
            "/media/fat/games/SNES/one.sfc",
            root.join("_@Favorites/SNES/One.sfc"),
        )
        .unwrap();
        let store = UserStateStore::open(root.join("user-state.sqlite3")).unwrap();
        let games = vec![game("one")];
        let report = import_legacy_snes(&store, &games, &root, 100).unwrap();
        assert_eq!(report.favourites_imported, 1);
        assert_eq!(store.favourite_count("snes").unwrap(), 1);
    }
}
