// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::agent_client;
use rusqlite::{Connection, OpenFlags};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

pub const LIBRARY_PAGE_SIZE: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibrarySortColumn {
    Title,
    System,
    Year,
    Manufacturer,
    Category,
    Preview,
    Discovered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibrarySortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryQuery {
    pub search: String,
    pub system: String,
    pub category: String,
    pub manufacturer: String,
    pub region: String,
    pub preview: String,
    pub confidence: String,
    pub sort_column: LibrarySortColumn,
    pub sort_direction: LibrarySortDirection,
    pub page: usize,
    pub page_size: usize,
}

impl Default for LibraryQuery {
    fn default() -> Self {
        Self {
            search: String::new(),
            system: String::new(),
            category: String::new(),
            manufacturer: String::new(),
            region: String::new(),
            preview: String::new(),
            confidence: String::new(),
            sort_column: LibrarySortColumn::Title,
            sort_direction: LibrarySortDirection::Ascending,
            page: 1,
            page_size: LIBRARY_PAGE_SIZE,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibraryCatalog {
    pub games: Vec<LibraryGame>,
    pub systems: Vec<String>,
    pub categories: Vec<String>,
    pub manufacturers: Vec<String>,
    pub regions: Vec<String>,
    pub confidences: Vec<String>,
    pub source_path: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibraryView {
    pub rows: Vec<LibraryGame>,
    pub total_count: usize,
    pub page: usize,
    pub page_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibraryGame {
    pub id: String,
    pub launch_id: i64,
    pub title: String,
    pub sort_title: String,
    pub system_id: String,
    pub system_title: String,
    pub category: String,
    pub manufacturer: String,
    pub year: String,
    pub discovered_at_unix: String,
    pub preview_asset_key: String,
    pub has_preview: bool,
    pub launch_kind: String,
    pub launch_ref: String,
    pub source_path: String,
    pub payload_path: String,
    pub core_id: String,
    pub hardware_id: String,
    pub setname: String,
    pub parent: String,
    pub confidence: String,
    pub region: String,
    pub region_confidence: String,
    pub identities: Vec<LibraryIdentity>,
    search_text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibraryIdentity {
    pub namespace: String,
    pub identity_id: String,
    pub family_id: String,
    pub metadata_title: String,
    pub year: String,
    pub manufacturer: String,
    pub category: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibrarySyncResult {
    pub catalog: LibraryCatalog,
    pub status: String,
    pub warning: String,
}

pub fn sync_library_catalog(host: &str) -> Result<LibrarySyncResult, String> {
    let cache_path = library_cache_path(host)?;
    match agent_client::fetch_library_database_snapshot(host) {
        Ok(snapshot) => {
            write_snapshot_atomically(&cache_path, &snapshot.bytes)?;
            let catalog = load_library_catalog(&cache_path)?;
            Ok(LibrarySyncResult {
                status: format!(
                    "Synced {} games from {}.",
                    catalog.games.len(),
                    snapshot.remote_path
                ),
                warning: String::new(),
                catalog,
            })
        }
        Err(err) if cache_path.is_file() => {
            let catalog = load_library_catalog(&cache_path)?;
            Ok(LibrarySyncResult {
                status: format!(
                    "Using cached Library copy with {} games.",
                    catalog.games.len()
                ),
                warning: format!("Live sync failed: {err}"),
                catalog,
            })
        }
        Err(err) => Err(format!("sync library database: {err}")),
    }
}

pub fn load_library_catalog(path: &Path) -> Result<LibraryCatalog, String> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| format!("open library database {}: {err}", path.display()))?;
    let identities = load_identities(&conn)?;
    let mut games = load_games(&conn, identities)?;
    games.sort_by(|a, b| natural_cmp(&a.sort_title, &b.sort_title));

    let mut systems = BTreeSet::new();
    let mut categories = BTreeSet::new();
    let mut manufacturers = BTreeSet::new();
    let mut regions = BTreeSet::new();
    let mut confidences = BTreeSet::new();
    for game in &games {
        insert_nonempty(&mut systems, &game.system_title);
        insert_nonempty(&mut categories, &game.category);
        insert_nonempty(&mut manufacturers, &game.manufacturer);
        insert_nonempty(&mut regions, &game.region);
        insert_nonempty(&mut confidences, &game.confidence);
    }

    Ok(LibraryCatalog {
        games,
        systems: systems.into_iter().collect(),
        categories: categories.into_iter().collect(),
        manufacturers: manufacturers.into_iter().collect(),
        regions: regions.into_iter().collect(),
        confidences: confidences.into_iter().collect(),
        source_path: path.to_path_buf(),
    })
}

pub fn apply_library_query(catalog: &LibraryCatalog, query: &LibraryQuery) -> LibraryView {
    let search_terms = query
        .search
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let mut rows = catalog
        .games
        .iter()
        .filter(|game| {
            search_terms
                .iter()
                .all(|term| game.search_text.contains(term))
        })
        .filter(|game| filter_matches(&query.system, &game.system_title))
        .filter(|game| filter_matches(&query.category, &game.category))
        .filter(|game| filter_matches(&query.manufacturer, &game.manufacturer))
        .filter(|game| filter_matches(&query.region, &game.region))
        .filter(|game| filter_matches(&query.confidence, &game.confidence))
        .filter(|game| match query.preview.as_str() {
            "" => true,
            "with-preview" => game.has_preview,
            "missing-preview" => !game.has_preview,
            _ => true,
        })
        .cloned()
        .collect::<Vec<_>>();

    sort_games(&mut rows, query.sort_column, query.sort_direction);

    let total_count = rows.len();
    let page_size = query.page_size.max(1);
    let page_count = total_count.div_ceil(page_size).max(1);
    let page = query.page.clamp(1, page_count);
    let start = (page - 1) * page_size;
    let rows = rows.into_iter().skip(start).take(page_size).collect();
    LibraryView {
        rows,
        total_count,
        page,
        page_count,
    }
}

pub fn selected_game<'a>(catalog: &'a LibraryCatalog, id: &str) -> Option<&'a LibraryGame> {
    catalog.games.iter().find(|game| game.id == id)
}

pub fn safe_host_component(host: &str) -> String {
    let mut out = String::with_capacity(host.len());
    for ch in host.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out == "." || out == ".." {
        "host".to_string()
    } else {
        out
    }
}

fn library_cache_path(host: &str) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set; cannot locate Library cache directory".to_string())?;
    Ok(home
        .join("Library/Caches/MiSTer MagiK/library")
        .join(safe_host_component(host))
        .join("library.sqlite3"))
}

fn write_snapshot_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("cache path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("create library cache {}: {err}", parent.display()))?;
    let temp = path.with_extension("sqlite3.tmp");
    fs::write(&temp, bytes).map_err(|err| format!("write library cache temp: {err}"))?;
    fs::rename(&temp, path).map_err(|err| {
        let _ = fs::remove_file(&temp);
        format!("publish library cache {}: {err}", path.display())
    })
}

fn load_games(
    conn: &Connection,
    identities: HashMap<String, Vec<LibraryIdentity>>,
) -> Result<Vec<LibraryGame>, String> {
    let mut stmt = conn
        .prepare(
            r#"
            SELECT
                lp.game_id,
                lc.launch_id,
                lc.title,
                lc.sort_title,
                lc.system_id,
                COALESCE(systems.title, lc.system_id) AS system_title,
                COALESCE(lc.category, '') AS category,
                COALESCE(lc.manufacturer, '') AS manufacturer,
                COALESCE(CAST(lc.year AS TEXT), '') AS year,
                COALESCE(CAST(lc.discovered_at_unix AS TEXT), '') AS discovered_at_unix,
                COALESCE(lc.preview_asset_key, '') AS preview_asset_key,
                lc.has_preview,
                COALESCE(lp.launch_kind, '') AS launch_kind,
                COALESCE(lc.launch_ref, '') AS launch_ref,
                COALESCE(lp.source_path, '') AS source_path,
                COALESCE(lp.payload_path, '') AS payload_path,
                COALESCE(lp.core_id, '') AS core_id,
                COALESCE(lp.hardware_id, '') AS hardware_id,
                COALESCE(lp.setname, '') AS setname,
                COALESCE(lp.parent, '') AS parent,
                COALESCE(lp.confidence, '') AS confidence,
                COALESCE(region_metadata.inferred_region, '') AS inferred_region,
                COALESCE(region_metadata.confidence, '') AS region_confidence
            FROM launcher_catalog_text lc
            JOIN launch_plans lp ON lp.launch_id = lc.launch_id
            LEFT JOIN systems ON systems.system_id = lc.system_id
            LEFT JOIN region_metadata ON region_metadata.game_id = lp.game_id
            "#,
        )
        .map_err(|err| format!("prepare library games query: {err}"))?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let identities = identities.get(&id).cloned().unwrap_or_default();
            let mut game = LibraryGame {
                id,
                launch_id: row.get(1)?,
                title: row.get(2)?,
                sort_title: row.get(3)?,
                system_id: row.get(4)?,
                system_title: row.get(5)?,
                category: row.get(6)?,
                manufacturer: row.get(7)?,
                year: row.get::<_, String>(8)?,
                discovered_at_unix: row.get::<_, String>(9)?,
                preview_asset_key: row.get(10)?,
                has_preview: row.get::<_, i64>(11)? != 0,
                launch_kind: row.get(12)?,
                launch_ref: row.get(13)?,
                source_path: row.get(14)?,
                payload_path: row.get(15)?,
                core_id: row.get(16)?,
                hardware_id: row.get(17)?,
                setname: row.get(18)?,
                parent: row.get(19)?,
                confidence: row.get(20)?,
                region: row.get(21)?,
                region_confidence: row.get(22)?,
                identities,
                search_text: String::new(),
            };
            game.search_text = build_search_text(&game);
            Ok(game)
        })
        .map_err(|err| format!("query library games: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("read library game row: {err}"))
}

fn load_identities(conn: &Connection) -> Result<HashMap<String, Vec<LibraryIdentity>>, String> {
    let mut stmt = conn
        .prepare(
            r#"
            SELECT
                launchable_id,
                COALESCE(namespace, ''),
                COALESCE(identity_id, ''),
                COALESCE(family_id, ''),
                COALESCE(metadata_title, ''),
                COALESCE(year, ''),
                COALESCE(manufacturer, ''),
                COALESCE(category, ''),
                COALESCE(source, '')
            FROM launchable_identities
            "#,
        )
        .map_err(|err| format!("prepare launchable identities query: {err}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                LibraryIdentity {
                    namespace: row.get(1)?,
                    identity_id: row.get(2)?,
                    family_id: row.get(3)?,
                    metadata_title: row.get(4)?,
                    year: row.get(5)?,
                    manufacturer: row.get(6)?,
                    category: row.get(7)?,
                    source: row.get(8)?,
                },
            ))
        })
        .map_err(|err| format!("query launchable identities: {err}"))?;
    let mut by_game = HashMap::new();
    for row in rows {
        let (game_id, identity) = row.map_err(|err| format!("read identity row: {err}"))?;
        by_game
            .entry(game_id)
            .or_insert_with(Vec::new)
            .push(identity);
    }
    Ok(by_game)
}

fn build_search_text(game: &LibraryGame) -> String {
    let mut parts = vec![
        game.title.as_str(),
        game.system_title.as_str(),
        game.system_id.as_str(),
        game.manufacturer.as_str(),
        game.category.as_str(),
        game.region.as_str(),
        game.year.as_str(),
        game.launch_ref.as_str(),
        game.confidence.as_str(),
    ];
    for identity in &game.identities {
        parts.push(identity.namespace.as_str());
        parts.push(identity.identity_id.as_str());
        parts.push(identity.family_id.as_str());
        parts.push(identity.metadata_title.as_str());
        parts.push(identity.manufacturer.as_str());
        parts.push(identity.category.as_str());
        parts.push(identity.year.as_str());
        parts.push(identity.source.as_str());
    }
    parts.join(" ").to_ascii_lowercase()
}

fn filter_matches(filter: &str, value: &str) -> bool {
    filter.is_empty() || filter == value
}

fn insert_nonempty(set: &mut BTreeSet<String>, value: &str) {
    if !value.trim().is_empty() {
        set.insert(value.to_string());
    }
}

fn sort_games(
    games: &mut [LibraryGame],
    column: LibrarySortColumn,
    direction: LibrarySortDirection,
) {
    games.sort_by(|a, b| {
        let ordering = match column {
            LibrarySortColumn::Title => natural_cmp(&a.sort_title, &b.sort_title),
            LibrarySortColumn::System => a
                .system_title
                .cmp(&b.system_title)
                .then_with(|| natural_cmp(&a.sort_title, &b.sort_title)),
            LibrarySortColumn::Year => numeric_sort_key(&a.year)
                .cmp(&numeric_sort_key(&b.year))
                .then_with(|| natural_cmp(&a.sort_title, &b.sort_title)),
            LibrarySortColumn::Manufacturer => a
                .manufacturer
                .cmp(&b.manufacturer)
                .then_with(|| natural_cmp(&a.sort_title, &b.sort_title)),
            LibrarySortColumn::Category => a
                .category
                .cmp(&b.category)
                .then_with(|| natural_cmp(&a.sort_title, &b.sort_title)),
            LibrarySortColumn::Preview => a
                .has_preview
                .cmp(&b.has_preview)
                .then_with(|| natural_cmp(&a.sort_title, &b.sort_title)),
            LibrarySortColumn::Discovered => numeric_sort_key(&a.discovered_at_unix)
                .cmp(&numeric_sort_key(&b.discovered_at_unix))
                .then_with(|| natural_cmp(&a.sort_title, &b.sort_title)),
        };
        match direction {
            LibrarySortDirection::Ascending => ordering,
            LibrarySortDirection::Descending => ordering.reverse(),
        }
    });
}

fn numeric_sort_key(value: &str) -> i64 {
    value.parse::<i64>().unwrap_or(i64::MIN)
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a_chars = a.char_indices().peekable();
    let mut b_chars = b.char_indices().peekable();
    loop {
        match (a_chars.peek().copied(), b_chars.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some((_, ac)), Some((_, bc))) if ac.is_ascii_digit() && bc.is_ascii_digit() => {
                let a_digits = take_ascii_digits(a, &mut a_chars);
                let b_digits = take_ascii_digits(b, &mut b_chars);
                let a_number = a_digits.trim_start_matches('0');
                let b_number = b_digits.trim_start_matches('0');
                let a_number = if a_number.is_empty() { "0" } else { a_number };
                let b_number = if b_number.is_empty() { "0" } else { b_number };
                let ordering = a_number
                    .len()
                    .cmp(&b_number.len())
                    .then_with(|| a_number.cmp(b_number))
                    .then_with(|| a_digits.len().cmp(&b_digits.len()));
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some((_, ac)), Some((_, bc))) => {
                a_chars.next();
                b_chars.next();
                let ordering = ac.to_ascii_lowercase().cmp(&bc.to_ascii_lowercase());
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

fn take_ascii_digits<'a>(
    text: &'a str,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'a>>,
) -> &'a str {
    let start = chars.peek().map(|(index, _)| *index).unwrap_or(text.len());
    let mut end = start;
    while let Some((index, ch)) = chars.peek().copied() {
        if !ch.is_ascii_digit() {
            break;
        }
        end = index + ch.len_utf8();
        chars.next();
    }
    &text[start..end]
}

#[cfg(test)]
pub(crate) fn write_snapshot_atomically_for_test(path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_snapshot_atomically(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_client;
    use rusqlite::params;

    #[test]
    fn safe_host_component_prevents_path_escape() {
        assert_eq!(safe_host_component("192.168.1.117"), "192.168.1.117");
        assert_eq!(safe_host_component("../bad/host"), ".._bad_host");
        assert_eq!(safe_host_component(""), "host");
        assert_eq!(safe_host_component(".."), "host");
    }

    #[test]
    fn public_sync_entrypoint_is_referenced_for_intermediate_commit() {
        type SnapshotFn =
            fn(&str) -> Result<agent_client::LibraryDatabaseSnapshot, agent_client::AgentError>;
        let _sync: fn(&str) -> Result<LibrarySyncResult, String> = sync_library_catalog;
        let _snapshot: SnapshotFn = agent_client::fetch_library_database_snapshot;
        let _columns = [
            LibrarySortColumn::Title,
            LibrarySortColumn::System,
            LibrarySortColumn::Year,
            LibrarySortColumn::Manufacturer,
            LibrarySortColumn::Category,
            LibrarySortColumn::Preview,
            LibrarySortColumn::Discovered,
        ];
    }

    #[test]
    fn write_snapshot_atomically_publishes_bytes() {
        let root = temp_dir("library-cache");
        let path = root.join("nested/library.sqlite3");
        write_snapshot_atomically_for_test(&path, b"db").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"db");
        write_snapshot_atomically_for_test(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn loads_fixture_catalog_and_details() {
        let root = temp_dir("library-load");
        let db = root.join("library.sqlite3");
        write_fixture_db(&db);
        let catalog = load_library_catalog(&db).unwrap();

        assert_eq!(catalog.games.len(), 3);
        assert_eq!(
            catalog.systems,
            vec!["Arcade", "Nintendo Entertainment System"]
        );
        assert_eq!(catalog.categories, vec!["Platform", "Shooter"]);
        assert_eq!(catalog.manufacturers, vec!["Capcom", "Nintendo"]);
        assert_eq!(catalog.regions, vec!["Japan", "USA"]);

        let mario = selected_game(&catalog, "nes/mario").unwrap();
        assert_eq!(mario.title, "Super Mario Bros.");
        assert_eq!(mario.system_title, "Nintendo Entertainment System");
        assert_eq!(mario.launch_kind, "direct");
        assert_eq!(mario.source_path, "/media/fat/games/NES/Mario.nes");
        assert_eq!(mario.payload_path, "/media/fat/games/NES/Mario.nes");
        assert_eq!(mario.region, "USA");
        assert_eq!(mario.identities.len(), 1);
        assert_eq!(mario.identities[0].namespace, "mame-software");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn query_filters_sorts_and_pages() {
        let root = temp_dir("library-query");
        let db = root.join("library.sqlite3");
        write_fixture_db(&db);
        let catalog = load_library_catalog(&db).unwrap();

        let query = LibraryQuery {
            search: "mario usa".to_string(),
            ..LibraryQuery::default()
        };
        let view = apply_library_query(&catalog, &query);
        assert_eq!(view.total_count, 1);
        assert_eq!(view.rows[0].id, "nes/mario");

        let query = LibraryQuery {
            system: "Arcade".to_string(),
            preview: "missing-preview".to_string(),
            ..LibraryQuery::default()
        };
        let view = apply_library_query(&catalog, &query);
        assert_eq!(view.total_count, 1);
        assert_eq!(view.rows[0].id, "arcade/1943");

        let query = LibraryQuery {
            sort_column: LibrarySortColumn::Year,
            sort_direction: LibrarySortDirection::Descending,
            page: 2,
            page_size: 1,
            ..LibraryQuery::default()
        };
        let view = apply_library_query(&catalog, &query);
        assert_eq!(view.page, 2);
        assert_eq!(view.page_count, 3);
        assert_eq!(view.rows[0].title, "1943");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn title_sort_orders_numbered_games_naturally() {
        let mut catalog = LibraryCatalog {
            games: ["Game 10", "Game 2", "Game 1"]
                .into_iter()
                .map(|title| LibraryGame {
                    id: title.to_string(),
                    title: title.to_string(),
                    sort_title: title.to_string(),
                    ..LibraryGame::default()
                })
                .collect(),
            ..LibraryCatalog::default()
        };
        for game in &mut catalog.games {
            game.search_text = build_search_text(game);
        }

        let view = apply_library_query(&catalog, &LibraryQuery::default());
        let titles = view
            .rows
            .iter()
            .map(|game| game.title.as_str())
            .collect::<Vec<_>>();

        assert_eq!(titles, ["Game 1", "Game 2", "Game 10"]);
    }

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-desktop-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_fixture_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE systems(system_id TEXT PRIMARY KEY, title TEXT NOT NULL, category TEXT NOT NULL) WITHOUT ROWID;
            CREATE TABLE launcher_catalog(
                ordinal INTEGER PRIMARY KEY,
                launch_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                sort_title TEXT NOT NULL,
                preview_asset_key TEXT NOT NULL,
                has_preview INTEGER NOT NULL,
                system_id TEXT NOT NULL,
                year INTEGER,
                manufacturer TEXT,
                category TEXT,
                discovered_at_unix INTEGER
            );
            CREATE TABLE launch_plans(
                plan_id TEXT,
                launch_id INTEGER,
                game_id TEXT,
                profile_id TEXT,
                launch_kind TEXT,
                source_path TEXT,
                launch_ref TEXT,
                launcher_path TEXT,
                payload_path TEXT,
                core_id TEXT,
                hardware_id TEXT,
                setname TEXT,
                parent TEXT,
                confidence TEXT
            );
            CREATE VIEW launcher_catalog_text AS
                SELECT launcher_catalog.*, launch_plans.launch_ref AS launch_ref
                FROM launcher_catalog
                JOIN launch_plans ON launch_plans.launch_id = launcher_catalog.launch_id;
            CREATE TABLE region_metadata(game_id TEXT PRIMARY KEY, inferred_region TEXT, confidence TEXT) WITHOUT ROWID;
            CREATE TABLE launchable_identities(
                launchable_id TEXT,
                namespace TEXT,
                identity_id TEXT,
                family_id TEXT,
                metadata_title TEXT,
                year TEXT,
                manufacturer TEXT,
                category TEXT,
                source TEXT
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO systems(system_id,title,category) VALUES (?1,?2,?3)",
            params!["arcade", "Arcade", "Arcade"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO systems(system_id,title,category) VALUES (?1,?2,?3)",
            params!["nes", "Nintendo Entertainment System", "Console"],
        )
        .unwrap();
        insert_game(
            &conn,
            1,
            "arcade/1943",
            "1943",
            "1943",
            "arcade",
            1987,
            "Capcom",
            "Shooter",
            10,
            "",
            0,
            "direct",
            "/media/fat/_Arcade/1943.mra",
            "/media/fat/_Arcade/1943.mra",
            "",
            "_Arcade/1943",
            "MiSTer",
            "1943",
            "",
            "medium",
            "Japan",
        );
        insert_game(
            &conn,
            2,
            "arcade/forgotten-worlds",
            "Forgotten Worlds",
            "Forgotten Worlds",
            "arcade",
            1988,
            "Capcom",
            "Shooter",
            20,
            "arcade/forgotten",
            1,
            "direct",
            "/media/fat/_Arcade/Forgotten Worlds.mra",
            "/media/fat/_Arcade/Forgotten Worlds.mra",
            "",
            "_Arcade/Forgotten Worlds",
            "MiSTer",
            "forgottn",
            "",
            "high",
            "USA",
        );
        insert_game(
            &conn,
            3,
            "nes/mario",
            "Super Mario Bros.",
            "Super Mario Bros.",
            "nes",
            1985,
            "Nintendo",
            "Platform",
            30,
            "nes/mario",
            1,
            "direct",
            "/media/fat/games/NES/Mario.nes",
            "/media/fat/games/NES/Mario.nes",
            "/media/fat/games/NES/Mario.nes",
            "_Console/NES",
            "NES",
            "",
            "",
            "high",
            "USA",
        );
        conn.execute(
            "INSERT INTO launchable_identities VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                "nes/mario",
                "mame-software",
                "smb",
                "mario",
                "Super Mario Bros.",
                "1985",
                "Nintendo",
                "Platform",
                "software-list"
            ],
        )
        .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_game(
        conn: &Connection,
        launch_id: i64,
        game_id: &str,
        title: &str,
        sort_title: &str,
        system_id: &str,
        year: i64,
        manufacturer: &str,
        category: &str,
        discovered_at_unix: i64,
        preview_asset_key: &str,
        has_preview: i64,
        launch_kind: &str,
        launch_ref: &str,
        source_path: &str,
        payload_path: &str,
        core_id: &str,
        hardware_id: &str,
        setname: &str,
        parent: &str,
        confidence: &str,
        region: &str,
    ) {
        conn.execute(
            "INSERT INTO launcher_catalog VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                launch_id,
                launch_id,
                title,
                sort_title,
                preview_asset_key,
                has_preview,
                system_id,
                year,
                manufacturer,
                category,
                discovered_at_unix
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO launch_plans VALUES (?1,?2,?3,'',?4,?5,?6,'',?7,?8,?9,?10,?11,?12)",
            params![
                format!("plan:{game_id}"),
                launch_id,
                game_id,
                launch_kind,
                source_path,
                launch_ref,
                payload_path,
                core_id,
                hardware_id,
                setname,
                parent,
                confidence
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO region_metadata VALUES (?1,?2,'high')",
            params![game_id, region],
        )
        .unwrap();
    }
}
