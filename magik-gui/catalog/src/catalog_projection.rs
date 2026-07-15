// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Launcher catalog projection policy.
//!
//! Scanning, SQLite materialization, and runtime loading all need the same
//! product row rules: what is launchable, how variants collapse, how preview
//! assets attach, and how rows become launcher entries.

#[cfg(test)]
use crate::arcade_catalog;
use crate::arcade_catalog::{ArcadeCatalog, ArcadeGameEntry, ArcadeGameMetadataKey, LaunchTarget};
use crate::game_discovery::variant_score_from_haystack;
use crate::library_db;
use crate::prepared_collections::PreparedLaunchProvenance;
use crate::software_identity::ConsolePreviewAsset;
use rusqlite::{params, Transaction};
use std::collections::{BTreeMap, HashMap, HashSet};
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
    pub(crate) source_kind: String,
    pub(crate) setname: String,
    pub(crate) parent: String,
    pub(crate) family_key: Option<String>,
    pub(crate) prepared: Option<PreparedLaunchProvenance>,
}

#[derive(Clone, Debug)]
pub(crate) struct CatalogProjectionSource {
    pub(crate) source_kind: String,
    pub(crate) setname: String,
    pub(crate) parent: String,
    pub(crate) family_key: Option<String>,
    pub(crate) prepared: Option<PreparedLaunchProvenance>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArcadeCompatibilityRow {
    pub(crate) launch_id: i64,
    pub(crate) family_id: String,
    pub(crate) identity_id: Option<String>,
    pub(crate) title: String,
    pub(crate) system_id: String,
    pub(crate) launch_ref: String,
    pub(crate) preview_asset_key: String,
    pub(crate) has_preview: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CanonicalLauncherInsertStats {
    pub(crate) rows: usize,
    pub(crate) launch_plans: usize,
}

#[derive(Default)]
pub(crate) struct CanonicalLaunchIdIndex {
    by_ref: HashMap<String, HashMap<String, HashMap<String, i64>>>,
}

impl CanonicalLaunchIdIndex {
    pub(crate) fn insert(
        &mut self,
        launch_ref: String,
        title: &str,
        system_id: &str,
        launch_id: i64,
    ) {
        self.by_ref
            .entry(launch_ref)
            .or_default()
            .entry(title.to_string())
            .or_default()
            .insert(system_id.to_string(), launch_id);
    }

    fn get(&self, game: &ArcadeGameEntry) -> Option<i64> {
        self.by_ref
            .get(game.mra_path.as_ref())
            .and_then(|by_title| by_title.get(game.title.as_ref()))
            .and_then(|by_system| by_system.get(game.system_id.as_ref()))
            .copied()
    }

    pub(crate) fn len(&self) -> usize {
        self.by_ref
            .values()
            .flat_map(HashMap::values)
            .map(HashMap::len)
            .sum()
    }
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
            source_kind: source.source_kind,
            setname: source.setname,
            parent: source.parent,
            family_key: source.family_key,
            prepared: source.prepared,
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
    let mut rows = collapse_catalog_variant_rows(rows);
    sort_catalog_projection_rows(&mut rows);
    let games: Vec<ArcadeGameEntry> = rows.into_iter().map(|row| row.game).collect();
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

pub(crate) fn sort_catalog_projection_rows(rows: &mut [CatalogProjectionRow]) {
    rows.sort_by_cached_key(|row| {
        (
            row.game.title.to_ascii_lowercase(),
            u8::from(row.prepared.is_none()),
            row.game.mra_path.to_ascii_lowercase(),
        )
    });
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
    if row.prepared.is_some() {
        score += 10_000;
    }
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
            launch_id,
            preview_asset_key,
            has_preview,
            asset_link_reason,
            preferred,
            preferred_reason
        )
        WITH candidates AS (
            SELECT
                COALESCE(
                    i.family_id,
                    g.game_id
                ) AS family_id,
                g.game_id AS launchable_id,
                lt.launch_id AS launch_id,
                g.title AS title,
                lower(g.title) AS sort_title,
                CASE launch_ref_kind_values.value
                    WHEN 'payload' THEN 'magik-plan:payload:' || payload_paths.path
                    WHEN 'archive' THEN 'magik-plan:archive:' || payload_paths.path
                    WHEN 'same-payload' THEN payload_paths.path
                    ELSE launch_paths.path
                END AS launch_ref,
                g.system_id AS system_id,
                COALESCE(i.year, g.year) AS year,
                COALESCE(i.manufacturer, g.manufacturer) AS manufacturer,
                COALESCE(i.category, g.genre) AS category,
                g.discovered_at_unix AS discovered_at_unix,
                lt.setname AS setname,
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
            FROM launch_target_rows lt
            JOIN games g ON g.game_key_id = lt.launch_id
            JOIN string_values launch_ref_kind_values
              ON launch_ref_kind_values.string_id = lt.launch_ref_kind_string_id
            LEFT JOIN path_values_text launch_paths
              ON launch_paths.path_id = lt.launch_path_id
            LEFT JOIN path_values_text payload_paths
              ON payload_paths.path_id = lt.payload_path_id
            LEFT JOIN launchable_identities i
              ON i.game_key_id = lt.launch_id
             AND i.namespace = 'mame'
            WHERE g.system_id IN ('arcade','neogeo')
              AND (
                CASE launch_ref_kind_values.value
                    WHEN 'payload' THEN 'magik-plan:payload:' || payload_paths.path
                    WHEN 'archive' THEN 'magik-plan:archive:' || payload_paths.path
                    WHEN 'same-payload' THEN payload_paths.path
                    ELSE launch_paths.path
                END
              ) != ''
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
            launch_id,
            preview_key,
            preview_available,
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

/// Populate the retained SQLite compatibility projection from the exact
/// current-generation RAM catalog rather than rebuilding it through text
/// views that repeatedly decompress interned paths.
pub(crate) fn materialize_arcade_ui_projection_rows(
    tx: &Transaction<'_>,
    mut rows: Vec<ArcadeCompatibilityRow>,
    catalog: &ArcadeCatalog,
) -> Result<usize, String> {
    let preferred_games = catalog
        .games
        .iter()
        .filter(|game| matches!(game.system_id.as_ref(), "arcade" | "neogeo"))
        .collect::<Vec<_>>();
    let preferred_keys = preferred_games
        .iter()
        .map(|game| {
            (
                game.mra_path.to_string(),
                game.title.to_string(),
                game.system_id.to_string(),
            )
        })
        .collect::<HashSet<_>>();
    let preferred_games_by_key = preferred_games
        .iter()
        .map(|game| {
            (
                (
                    game.mra_path.to_string(),
                    game.title.to_string(),
                    game.system_id.to_string(),
                ),
                *game,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut grouped = BTreeMap::<String, Vec<ArcadeCompatibilityRow>>::new();
    for row in rows.drain(..) {
        grouped
            .entry(library_db::normalize_id(&row.family_id))
            .or_default()
            .push(row);
    }

    let mut variant_stmt = tx
        .prepare(
            "INSERT INTO ui_arcade_variants(
                family_id,variant_ordinal,launch_id,preview_asset_key,has_preview,
                asset_link_reason,preferred,preferred_reason
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        )
        .map_err(|e| format!("prepare Rust arcade variant insert: {e}"))?;
    let mut preferred_rows = HashMap::<(String, String, String), (String, i64)>::new();
    for (family_id, family_rows) in &mut grouped {
        family_rows.sort_by_cached_key(|row| {
            let key = (
                row.launch_ref.clone(),
                row.title.clone(),
                row.system_id.clone(),
            );
            (
                !preferred_keys.contains(&key),
                row.title.to_ascii_lowercase(),
                row.launch_ref.to_ascii_lowercase(),
            )
        });
        let preferred_count = family_rows
            .iter()
            .filter(|row| {
                preferred_keys.contains(&(
                    row.launch_ref.clone(),
                    row.title.clone(),
                    row.system_id.clone(),
                ))
            })
            .count();
        if preferred_count != 1 {
            return Err(format!(
                "canonical arcade family {family_id} has {preferred_count} preferred rows"
            ));
        }
        for (variant_ordinal, row) in family_rows.iter().enumerate() {
            let key = (
                row.launch_ref.clone(),
                row.title.clone(),
                row.system_id.clone(),
            );
            let preferred = preferred_keys.contains(&key);
            let preferred_reason = if preferred {
                if row
                    .identity_id
                    .as_deref()
                    .is_some_and(|identity| library_db::normalize_id(identity) == *family_id)
                {
                    "installed-parent"
                } else {
                    "deterministic-child"
                }
            } else {
                "variant"
            };
            let (preview_asset_key, has_preview) =
                if let Some(canonical) = preferred_games_by_key.get(&key) {
                    if row.preview_asset_key != canonical.preview_asset_key.as_ref()
                        || row.has_preview != canonical.has_preview
                    {
                        return Err(format!(
                            "canonical arcade preview mismatch for {}: prepared_key={} prepared_has_preview={} canonical_key={} canonical_has_preview={}",
                            canonical.mra_path,
                            row.preview_asset_key,
                            row.has_preview,
                            canonical.preview_asset_key,
                            canonical.has_preview
                        ));
                    }
                    (canonical.preview_asset_key.as_ref(), canonical.has_preview)
                } else {
                    (row.preview_asset_key.as_str(), row.has_preview)
                };
            variant_stmt
                .execute(params![
                    family_id,
                    variant_ordinal as i64,
                    row.launch_id,
                    preview_asset_key,
                    i64::from(has_preview),
                    if has_preview {
                        "derived-family"
                    } else {
                        "none"
                    },
                    i64::from(preferred),
                    preferred_reason,
                ])
                .map_err(|e| format!("insert Rust arcade variant: {e}"))?;
            if preferred {
                preferred_rows.insert(key, (family_id.clone(), variant_ordinal as i64));
            }
        }
    }
    drop(variant_stmt);

    if preferred_rows.len() != preferred_games.len() {
        return Err(format!(
            "canonical arcade projection row mismatch preferred={} catalog={}",
            preferred_rows.len(),
            preferred_games.len()
        ));
    }
    let mut preferred_stmt = tx
        .prepare(
            "INSERT INTO ui_arcade_preferred(ordinal,family_id,variant_ordinal)
             VALUES (?1,?2,?3)",
        )
        .map_err(|e| format!("prepare Rust arcade preferred insert: {e}"))?;
    for (ordinal, game) in preferred_games.iter().enumerate() {
        let key = (
            game.mra_path.to_string(),
            game.title.to_string(),
            game.system_id.to_string(),
        );
        let (family_id, variant_ordinal) = preferred_rows.get(&key).ok_or_else(|| {
            format!(
                "canonical arcade row is missing from compatibility data: {}",
                game.mra_path
            )
        })?;
        let _source = grouped
            .get(family_id)
            .and_then(|family| family.get(*variant_ordinal as usize))
            .ok_or_else(|| {
                format!(
                    "canonical arcade compatibility pointer is invalid: {}",
                    game.mra_path
                )
            })?;
        preferred_stmt
            .execute(params![ordinal as i64, family_id, variant_ordinal])
            .map_err(|e| format!("insert Rust arcade preferred row: {e}"))?;
    }
    Ok(preferred_games.len())
}

pub(crate) fn insert_canonical_launcher_catalog(
    tx: &Transaction<'_>,
    catalog: &ArcadeCatalog,
    launch_ids: &CanonicalLaunchIdIndex,
    ordinal_offset: usize,
) -> Result<CanonicalLauncherInsertStats, String> {
    let mut stmt = tx
        .prepare(
            "INSERT INTO launcher_catalog_rows(ordinal,launch_id,preview_asset_key,has_preview)
             VALUES (?1,?2,?3,?4)",
        )
        .map_err(|e| format!("prepare canonical launcher catalog insert: {e}"))?;
    let mut stats = CanonicalLauncherInsertStats::default();
    for game in catalog
        .games
        .iter()
        .filter(|game| !matches!(game.system_id.as_ref(), "arcade" | "neogeo"))
    {
        let launch_id = launch_ids.get(game).ok_or_else(|| {
            format!(
                "canonical launcher row has no source launch id: {}",
                game.mra_path
            )
        })?;
        stmt.execute(params![
            (ordinal_offset + stats.rows) as i64,
            launch_id,
            game.preview_asset_key.as_ref(),
            i64::from(game.has_preview),
        ])
        .map_err(|e| format!("insert canonical launcher catalog: {e}"))?;
        stats.rows += 1;
        if matches!(
            catalog.launch_target_for_ref(game.mra_path.as_ref()),
            LaunchTarget::Structured(_)
        ) {
            stats.launch_plans += 1;
        }
    }
    Ok(stats)
}

pub(crate) fn insert_arcade_launcher_catalog(tx: &Transaction<'_>) -> Result<(), String> {
    tx.query_row("SELECT count(*) FROM ui_arcade_preferred", [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|_| ())
    .map_err(|e| format!("count preferred launcher catalog: {e}"))
}

pub(crate) fn insert_console_launcher_catalog(
    tx: &Transaction<'_>,
    mut rows: Vec<CatalogProjectionRow>,
) -> Result<usize, String> {
    let ordinal_offset = tx
        .query_row(
            "SELECT
                (SELECT count(*) FROM ui_arcade_preferred)
                + (SELECT count(*) FROM launcher_catalog_rows)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| format!("query launcher catalog offset: {e}"))?;
    rows.sort_by_cached_key(|row| row.game.title.to_ascii_lowercase());
    let mut launcher_games = collapse_catalog_variant_rows(rows);
    sort_catalog_projection_rows(&mut launcher_games);
    let mut launcher_stmt = tx
        .prepare(
            "INSERT INTO launcher_catalog_rows(ordinal,launch_id,preview_asset_key,has_preview)
             VALUES (?1,?2,?3,?4)",
        )
        .map_err(|e| format!("prepare launcher catalog insert: {e}"))?;
    for (idx, row) in launcher_games.iter().enumerate() {
        let game = &row.game;
        launcher_stmt
            .execute(params![
                ordinal_offset + idx as i64,
                row.launch_id,
                game.preview_asset_key.as_ref(),
                if game.has_preview { 1 } else { 0 }
            ])
            .map_err(|e| format!("insert launcher catalog: {e}"))?;
    }
    Ok(launcher_games.len())
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
    use crate::prepared_collections::{PreparedCollectionId, PreparedLaunchProvenance};
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
    fn prepared_launch_sorts_before_generic_exact_title_without_collapsing() {
        let generic = catalog_entry_row("Doom", "/media/fat/games/DOS/Doom.vhd");
        let mut prepared = catalog_entry_row("Doom", "/media/fat/_DOS Games/Doom.mgl");
        prepared.prepared = Some(PreparedLaunchProvenance::prepared(
            PreparedCollectionId::ZeroMhz,
        ));
        let mut rows = vec![generic, prepared];

        sort_catalog_projection_rows(&mut rows);

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].game.mra_path.as_ref(),
            "/media/fat/_DOS Games/Doom.mgl"
        );
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
    fn collapsed_projection_rows_are_sorted_by_final_visible_title() {
        let rows = vec![
            CatalogProjectionRow {
                family_key: Some("mame-software:megadrive:another-world".to_string()),
                ..catalog_entry_row("Another World", "/z/Another World.md")
            },
            catalog_entry_row("Aq Renkan Awa", "/m/Aq Renkan Awa.md"),
            CatalogProjectionRow {
                family_key: Some("mame-software:megadrive:another-world".to_string()),
                ..catalog_entry_row("Out of This World", "/a/Out of This World.md")
            },
        ];

        let catalog = catalog_from_projection_rows("/media/fat/_Arcade", rows);
        let titles = catalog
            .system_games("amiga")
            .into_iter()
            .map(|game| game.title.to_string())
            .collect::<Vec<_>>();

        assert_eq!(titles, ["Aq Renkan Awa", "Out of This World"]);
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
