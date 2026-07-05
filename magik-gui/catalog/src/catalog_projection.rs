//! Launcher catalog projection policy.
//!
//! Scanning, SQLite materialization, and runtime loading all need the same
//! product row rules: what is launchable, how variants collapse, how preview
//! assets attach, and how rows become launcher entries.

#[cfg(test)]
use crate::arcade_catalog::{self, ArcadeCatalog};
use crate::arcade_catalog::{ArcadeGameEntry, ArcadeGameMetadataKey};
use crate::game_discovery::variant_score_from_haystack;
use crate::library_db;
use crate::software_identity::ConsolePreviewAsset;
use rusqlite::{params, Transaction};
use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LauncherPreviewAsset {
    pub(crate) archive_path: String,
    pub(crate) asset_key: String,
}

impl LauncherPreviewAsset {
    pub(crate) fn none() -> Self {
        Self::default()
    }

    pub(crate) fn new(archive_path: impl Into<String>, asset_key: impl Into<String>) -> Self {
        let archive_path = archive_path.into();
        let asset_key = asset_key.into();
        Self {
            archive_path,
            asset_key,
        }
    }

    pub(crate) fn from_console_asset(asset: Option<&ConsolePreviewAsset>) -> Self {
        asset
            .map(|asset| Self::new(asset.archive_path.clone(), asset.asset_key.to_string()))
            .unwrap_or_default()
    }

    pub(crate) fn has_preview(&self) -> bool {
        !self.archive_path.is_empty() && !self.asset_key.is_empty()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CatalogProjectionRow {
    pub(crate) launch_id: i64,
    pub(crate) game: ArcadeGameEntry,
    pub(crate) discovered_at_unix: Option<i64>,
    pub(crate) source_kind: String,
    pub(crate) setname: String,
    pub(crate) parent: String,
    pub(crate) family_key: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct CatalogProjectionSource {
    pub(crate) discovered_at_unix: Option<i64>,
    pub(crate) source_kind: String,
    pub(crate) setname: String,
    pub(crate) parent: String,
    pub(crate) family_key: Option<String>,
}

impl CatalogProjectionRow {
    pub(crate) fn new(
        title: impl Into<String>,
        launch_ref: impl Into<String>,
        system_id: impl Into<String>,
        preview: LauncherPreviewAsset,
        metadata: ArcadeGameMetadataKey,
        is_new: bool,
        source: CatalogProjectionSource,
    ) -> Self {
        let title = title.into();
        let launch_ref = launch_ref.into();
        let system_id = system_id.into();
        let has_preview = preview.has_preview();
        Self {
            launch_id: 0,
            game: launcher_entry(
                title,
                launch_ref,
                system_id,
                preview,
                metadata,
                has_preview,
                is_new,
            ),
            discovered_at_unix: source.discovered_at_unix,
            source_kind: source.source_kind,
            setname: source.setname,
            parent: source.parent,
            family_key: source.family_key,
        }
    }

    pub(crate) fn with_launch_id(mut self, launch_id: i64) -> Self {
        self.launch_id = launch_id;
        self
    }
}

pub(crate) fn launcher_entry(
    title: impl Into<String>,
    launch_ref: impl Into<String>,
    system_id: impl Into<String>,
    preview: LauncherPreviewAsset,
    metadata: ArcadeGameMetadataKey,
    has_preview: bool,
    is_new: bool,
) -> ArcadeGameEntry {
    ArcadeGameEntry {
        title: title.into().into(),
        mra_path: launch_ref.into().into(),
        preview_archive_path: preview.archive_path.into(),
        preview_asset_key: preview.asset_key.into(),
        has_preview,
        system_id: system_id.into().into(),
        year: metadata.year,
        manufacturer: metadata.manufacturer.into(),
        category: metadata.category.into(),
        is_new,
    }
}

#[cfg(test)]
pub(crate) fn catalog_from_projection_rows(
    root: impl AsRef<Path>,
    mut rows: Vec<CatalogProjectionRow>,
) -> ArcadeCatalog {
    rows.sort_by_cached_key(|row| row.game.title.to_ascii_lowercase());
    let games = collapse_catalog_variants(rows);
    let systems = arcade_catalog::systems_from_games(&games);
    ArcadeCatalog::new(root.as_ref().to_path_buf(), games, systems)
}

pub(crate) fn collapse_catalog_variants(rows: Vec<CatalogProjectionRow>) -> Vec<ArcadeGameEntry> {
    collapse_catalog_variant_rows(rows)
        .into_iter()
        .map(|row| row.game)
        .collect()
}

pub(crate) fn collapse_catalog_variant_rows(
    rows: Vec<CatalogProjectionRow>,
) -> Vec<CatalogProjectionRow> {
    let mut best_idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<CatalogProjectionRow> = Vec::with_capacity(rows.len());

    for row in rows {
        let key = catalog_variant_group_key(&row);
        if let Some(&idx) = best_idx.get(&key) {
            if prefer_catalog_variant(&row, &out[idx]) {
                out[idx] = row;
            }
        } else {
            best_idx.insert(key, out.len());
            out.push(row);
        }
    }

    out
}

fn catalog_variant_group_key(row: &CatalogProjectionRow) -> String {
    if let Some(family_key) = row.family_key.as_deref() {
        return format!("family:{}", library_db::normalize_id(family_key));
    }
    if row.source_kind == "mra" {
        if !row.setname.trim().is_empty() {
            let parent = row.parent.trim();
            let group = if parent.is_empty() {
                row.setname.as_str()
            } else {
                parent
            };
            return format!("mra:set:{}", library_db::normalize_id(group));
        }
        return format!("mra:title:{}", canonical_variant_title(&row.game.title));
    }
    if row.source_kind == "catalog-entry" {
        return format!(
            "catalog-entry:{}:{}",
            row.game.mra_path,
            library_db::normalize_id(&row.game.title)
        );
    }
    format!("{}:{}", row.source_kind, row.game.mra_path)
}

fn prefer_catalog_variant(a: &CatalogProjectionRow, b: &CatalogProjectionRow) -> bool {
    let a_score = catalog_variant_score(a);
    let b_score = catalog_variant_score(b);
    if a_score != b_score {
        return a_score > b_score;
    }
    if a.game.has_preview != b.game.has_preview {
        return a.game.has_preview;
    }
    a.game.mra_path < b.game.mra_path
}

fn catalog_variant_score(row: &CatalogProjectionRow) -> i32 {
    let haystack = format!(
        "{} {} {} {}",
        row.game.title, row.game.mra_path, row.setname, row.parent
    )
    .to_ascii_lowercase();

    let mut score = variant_score_from_haystack(&haystack);
    if row.source_kind == "mra" && !row.setname.trim().is_empty() && row.parent.trim().is_empty() {
        score += 1000;
    }
    score
}

pub(crate) fn canonical_variant_title(title: &str) -> String {
    let mut out = String::new();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    for ch in title.chars() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ if paren_depth == 0 && bracket_depth == 0 => out.push(ch),
            _ => {}
        }
    }
    library_db::normalize_id(
        out.trim_matches(|ch: char| ch.is_whitespace() || ch == '-' || ch == ','),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArcadePreviewProjection {
    arcade_archive_path: String,
    neogeo_archive_path: String,
}

impl ArcadePreviewProjection {
    pub(crate) fn new(
        arcade_archive_path: impl Into<String>,
        neogeo_archive_path: impl Into<String>,
    ) -> Self {
        Self {
            arcade_archive_path: arcade_archive_path.into(),
            neogeo_archive_path: neogeo_archive_path.into(),
        }
    }

    pub(crate) fn archive_for_system(&self, system_id: &str) -> &str {
        if system_id == "neogeo" {
            &self.neogeo_archive_path
        } else {
            &self.arcade_archive_path
        }
    }
}

pub(crate) fn materialize_arcade_ui_projections(
    tx: &Transaction<'_>,
    preview_projection: &ArcadePreviewProjection,
) -> Result<(), String> {
    tx.execute(
        r#"
        INSERT INTO ui_arcade_variants(
            family_id,
            variant_ordinal,
            launchable_id,
            launch_id,
            title,
            sort_title,
            preview_asset_key,
            has_preview,
            system_id,
            year,
            manufacturer,
            category,
            discovered_at_unix,
            identity_id,
            parent_setname,
            asset_key,
            asset_link_reason,
            preferred,
            preferred_reason
        )
        WITH candidates AS (
            SELECT
                COALESCE(i.family_id, l.launchable_id) AS family_id,
                l.launchable_id AS launchable_id,
                l.launch_id AS launch_id,
                l.title AS title,
                lower(l.title) AS sort_title,
                l.launch_ref AS launch_ref,
                l.system_id AS system_id,
                COALESCE(i.year, g.year) AS year,
                COALESCE(i.manufacturer, g.manufacturer) AS manufacturer,
                COALESCE(i.category, g.genre) AS category,
                g.discovered_at_unix AS discovered_at_unix,
                l.setname AS setname,
                i.identity_id AS identity_id,
                CASE
                    WHEN i.identity_id IS NOT NULL
                     AND i.family_id IS NOT NULL
                     AND i.identity_id != i.family_id
                    THEN i.family_id
                    ELSE NULL
                END AS parent_setname,
                CASE
                    WHEN i.identity_id IS NOT NULL
                     AND i.identity_id = COALESCE(i.family_id, i.identity_id)
                    THEN 1
                    ELSE 0
                END AS is_parent
            FROM launchables l
            JOIN games g ON g.game_id = l.launchable_id
            LEFT JOIN launchable_identities i
              ON i.launchable_id = l.launchable_id
             AND i.namespace = 'mame'
            WHERE l.system_id IN ('arcade','neogeo')
              AND l.launch_ref != ''
        ),
        resolved AS (
            SELECT
                *,
                CASE
                    WHEN system_id = 'neogeo' THEN ?2
                    ELSE ?1
                END AS preview_archive_path,
                CASE
                    WHEN system_id = 'neogeo' THEN COALESCE(NULLIF(setname, ''), '')
                    ELSE COALESCE(NULLIF(family_id, ''), NULLIF(identity_id, ''), NULLIF(setname, ''), '')
                END AS preview_key
            FROM candidates
        ),
        resolved_with_preview AS (
            SELECT
                *,
                CASE
                    WHEN preview_archive_path != '' AND preview_key != '' THEN 1
                    ELSE 0
                END AS preview_available
            FROM resolved
        ),
        ranked AS (
            SELECT
                *,
                row_number() OVER (
                    PARTITION BY family_id
                    ORDER BY is_parent DESC,
                             sort_title ASC,
                             launch_ref ASC
                ) AS family_rank,
                row_number() OVER (
                    PARTITION BY family_id
                    ORDER BY is_parent DESC,
                             sort_title ASC,
                             launch_ref ASC
                ) - 1 AS variant_ordinal
            FROM resolved_with_preview
        )
        SELECT
            family_id,
            variant_ordinal,
            launchable_id,
            launch_id,
            title,
            sort_title,
            preview_key,
            preview_available,
            system_id,
            year,
            manufacturer,
            category,
            discovered_at_unix,
            identity_id,
            parent_setname,
            preview_key,
            CASE WHEN preview_available = 1 THEN 'derived-family' ELSE 'none' END,
            CASE WHEN family_rank = 1 THEN 1 ELSE 0 END,
            CASE
                WHEN family_rank = 1 AND is_parent = 1 THEN 'installed-parent'
                WHEN family_rank = 1 THEN 'deterministic-child'
                ELSE 'variant'
            END
        FROM ranked
        ORDER BY family_id, variant_ordinal;
        "#,
        params![
            preview_projection.archive_for_system("arcade"),
            preview_projection.archive_for_system("neogeo")
        ],
    )
    .map_err(|e| format!("materialize arcade ui variants: {e}"))?;
    tx.execute(
        r#"
        INSERT INTO ui_arcade_preferred(
            ordinal,
            family_id,
            variant_ordinal
        )
        SELECT
            row_number() OVER (ORDER BY sort_title ASC, launch_ref ASC) - 1,
            family_id,
            variant_ordinal
        FROM ui_arcade_variants_text
        WHERE preferred = 1
        ORDER BY sort_title ASC, launch_ref ASC;
        "#,
        [],
    )
    .map(|_| ())
    .map_err(|e| format!("materialize arcade ui projections: {e}"))
}

pub(crate) fn insert_arcade_launcher_catalog(tx: &Transaction<'_>) -> Result<(), String> {
    tx.execute(
        "INSERT INTO launcher_catalog(ordinal,launch_id,title,sort_title,preview_asset_key,has_preview,system_id,year,manufacturer,category,discovered_at_unix)
         SELECT ordinal,launch_id,title,sort_title,preview_asset_key,has_preview,system_id,year,manufacturer,category,discovered_at_unix
         FROM ui_arcade_preferred_text
         ORDER BY ordinal",
        [],
    )
    .map(|_| ())
    .map_err(|e| format!("insert preferred launcher catalog: {e}"))
}

pub(crate) fn insert_console_launcher_catalog(
    tx: &Transaction<'_>,
    mut rows: Vec<CatalogProjectionRow>,
) -> Result<usize, String> {
    let ordinal_offset = tx
        .query_row("SELECT count(*) FROM launcher_catalog", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|e| format!("query launcher catalog offset: {e}"))?;
    rows.sort_by_cached_key(|row| row.game.title.to_ascii_lowercase());
    let launcher_games = collapse_catalog_variant_rows(rows);
    let mut launcher_stmt = tx
        .prepare(
            "INSERT INTO launcher_catalog(ordinal,launch_id,title,sort_title,preview_asset_key,has_preview,system_id,year,manufacturer,category,discovered_at_unix)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        )
        .map_err(|e| format!("prepare launcher catalog insert: {e}"))?;
    for (idx, row) in launcher_games.iter().enumerate() {
        let game = &row.game;
        launcher_stmt
            .execute(params![
                ordinal_offset + idx as i64,
                row.launch_id,
                game.title.as_ref(),
                library_db::normalize_title(&game.title),
                game.preview_asset_key.as_ref(),
                if game.has_preview { 1 } else { 0 },
                game.system_id.as_ref(),
                game.year.map(i64::from),
                non_empty_arc_str(&game.manufacturer),
                non_empty_arc_str(&game.category),
                row.discovered_at_unix
            ])
            .map_err(|e| format!("insert launcher catalog: {e}"))?;
    }
    Ok(launcher_games.len())
}

fn non_empty_arc_str(value: &std::sync::Arc<str>) -> Option<&str> {
    if value.is_empty() {
        None
    } else {
        Some(value.as_ref())
    }
}

pub(crate) fn materialize_launcher_launch_plans(tx: &Transaction<'_>) -> Result<usize, String> {
    tx.query_row("SELECT count(*) FROM launcher_launch_plans", [], |row| {
        let count: i64 = row.get(0)?;
        Ok(count as usize)
    })
    .map_err(|e| format!("count launcher launch plans: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    #[test]
    fn catalog_variants_group_by_parent_and_prefer_us_release() {
        let rows = vec![
            catalog_row(
                "Moon Patrol (Japan)",
                "/media/fat/_Arcade/Moon Patrol (Japan).mra",
                "mpatrolj",
                "mpatrol",
            ),
            catalog_row(
                "Moon Patrol (prototype)",
                "/media/fat/_Arcade/Moon Patrol (prototype).mra",
                "mpatrolp",
                "mpatrol",
            ),
            catalog_row(
                "Moon Patrol (US)",
                "/media/fat/_Arcade/Moon Patrol (US).mra",
                "mpatrol",
                "",
            ),
        ];

        let games = collapse_catalog_variants(rows);

        assert_eq!(games.len(), 1);
        assert_eq!(games[0].title.as_ref(), "Moon Patrol (US)");
    }

    #[test]
    fn catalog_variants_keep_non_mra_launchers_separate() {
        let rows = vec![
            catalog_launcher_row("Amiga", "/media/fat/_Computer/Amiga.mgl"),
            catalog_launcher_row("Amiga 500", "/media/fat/_Computer/Amiga 500.mgl"),
        ];

        let games = collapse_catalog_variants(rows);

        assert_eq!(games.len(), 2);
    }

    #[test]
    fn catalog_entries_with_shared_collection_launch_ref_stay_separate() {
        let rows = vec![
            catalog_entry_row("Agony", "/media/fat/games/Amiga/AmigaVision-MiSTer.7z"),
            catalog_entry_row(
                "Alien Breed",
                "/media/fat/games/Amiga/AmigaVision-MiSTer.7z",
            ),
        ];

        let games = collapse_catalog_variants(rows);

        assert_eq!(games.len(), 2);
        assert!(games.iter().any(|game| game.title.as_ref() == "Agony"));
        assert!(games
            .iter()
            .any(|game| game.title.as_ref() == "Alien Breed"));
    }

    #[test]
    fn arcade_preview_projection_selects_neogeo_archive_by_system() {
        let projection = ArcadePreviewProjection::new("arcade-pack.raw565", "neogeo-pack.raw565");

        assert_eq!(
            projection.archive_for_system("arcade"),
            "arcade-pack.raw565"
        );
        assert_eq!(
            projection.archive_for_system("neogeo"),
            "neogeo-pack.raw565"
        );
    }
}
