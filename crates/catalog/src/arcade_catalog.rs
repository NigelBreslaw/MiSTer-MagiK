// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Arcade catalog helpers.
//!
//! The runtime launcher catalog is SQLite-backed. This module keeps the shared
//! in-memory catalog types and presentation helpers used by the SQLite loader.

pub use crate::catalog_classify::PlatformKind;
use crate::catalog_navigation::CatalogNavigationProjection;
use crate::library_db::{AMIGAVISION_GAME_LAUNCH_PREFIX, AMIGAVISION_LAUNCHER_REF};
use crate::prepared_collections::PreparedCollectionId;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

pub const DEFAULT_ARCADE_ROOT: &str = "/media/fat/_Arcade";

/// Logical row height for the Rust-painted arcade list viewport.
pub const ARCADE_ROW_HEIGHT: i32 = 48;
/// Visible list height: 10 exact arcade rows (matches the Rust-painted viewport).
pub const ARCADE_LIST_VISIBLE_H: i32 = ARCADE_ROW_HEIGHT * 10;
/// Canonical nested Home rail geometry at the original 960-wide layout.
/// Presentation scales this stable navigation coordinate to the live viewport.
pub const HOME_TILE_WIDTH: i32 = 191;
pub const HOME_TILE_GAP: i32 = 16;
/// Canonical Home list width inside the 18px left/right padding of the 960px UI.
pub const HOME_LIST_VISIBLE_W: i32 = 924;
pub const MENU_ARCADE_SYSTEM_ID: &str = "menu:arcade";

#[derive(Clone, Debug)]
pub struct ArcadeGameEntry {
    pub title: Arc<str>,
    pub mra_path: Arc<str>,
    pub preview_archive_path: Arc<str>,
    pub preview_asset_key: Arc<str>,
    pub has_preview: bool,
    pub system_id: Arc<str>,
    pub year: Option<u16>,
    pub manufacturer: Arc<str>,
    pub category: Arc<str>,
    pub players: Option<u8>,
    pub control: Arc<str>,
    pub is_new: bool,
}

impl ArcadeGameEntry {
    pub fn stable_key(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}",
            self.system_id, self.title, self.mra_path
        )
    }

    pub fn metadata_key(&self) -> ArcadeGameMetadataKey {
        ArcadeGameMetadataKey {
            year: self.year,
            manufacturer: self.manufacturer.to_string(),
            category: self.category.to_string(),
            players: self.players,
            control: self.control.to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArcadeGameMetadataKey {
    pub year: Option<u16>,
    pub manufacturer: String,
    pub category: String,
    pub players: Option<u8>,
    pub control: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameSystemEntry {
    pub id: String,
    pub title: String,
    pub count: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemProjectionStats {
    pub source_games: usize,
    pub visible_families: usize,
    pub collapsed_variants: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ArcadeFilter {
    All,
    Search,
    Category(String),
    Decade(u16),
    Manufacturer(String),
    Players(u8),
    Control(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArcadeFilterOption {
    pub label: String,
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct ArcadeCatalog {
    pub root: PathBuf,
    pub games: Arc<Vec<ArcadeGameEntry>>,
    pub systems: Vec<GameSystemEntry>,
    platform_kinds: Arc<HashMap<String, PlatformKind>>,
    games_by_system: Arc<HashMap<String, Vec<usize>>>,
    games_by_filter: Arc<HashMap<ArcadeFilterKey, Vec<usize>>>,
    filter_options_by_system: Arc<HashMap<String, ArcadeSystemFilterOptions>>,
    preview_games_by_system: Arc<HashMap<String, Vec<usize>>>,
    launch_plans_by_ref: Arc<HashMap<Arc<str>, StructuredLaunchPlan>>,
    projection_stats_by_system: Arc<HashMap<String, SystemProjectionStats>>,
    search_keys: Arc<Vec<ArcadeSearchKey>>,
    autocomplete: Arc<ArcadeAutocompleteIndex>,
    lazy_text_indexes: Arc<OnceLock<ArcadeTextIndexes>>,
    system_collections: Arc<HashMap<String, Arc<SystemCollection>>>,
}

/// Immutable navigation segment for one hydrated system.
///
/// The complete row set and its navigation indexes are built on CPU0 before publication. Catalog
/// adoption then shares this segment instead of cloning resident rows into a new monolithic
/// catalog.
#[derive(Clone, Debug)]
pub struct SystemCollection {
    system_id: Arc<str>,
    games: SystemCollectionRows,
    cold_metadata: Arc<Vec<ArcadeGameMetadataKey>>,
    platform_kinds: Arc<HashMap<String, PlatformKind>>,
    preview_games_by_system: Arc<HashMap<String, Vec<usize>>>,
    launch_plans: Arc<Vec<StructuredLaunchPlan>>,
    rich_indexes: Arc<OnceLock<ArcadeCatalogIndexes>>,
}

#[derive(Clone, Debug)]
enum SystemCollectionRows {
    Owned(Arc<Vec<ArcadeGameEntry>>),
    NavPack(Arc<NavPackSystemRows>),
}

impl SystemCollectionRows {
    fn len(&self) -> usize {
        match self {
            Self::Owned(games) => games.len(),
            Self::NavPack(games) => games.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get(&self, index: usize) -> Option<&ArcadeGameEntry> {
        match self {
            Self::Owned(games) => games.get(index),
            Self::NavPack(games) => games.get(index),
        }
    }

    fn iter(&self) -> SystemCollectionRowsIter<'_> {
        SystemCollectionRowsIter {
            rows: self,
            ordinal: 0,
        }
    }
}

struct SystemCollectionRowsIter<'a> {
    rows: &'a SystemCollectionRows,
    ordinal: usize,
}

impl<'a> Iterator for SystemCollectionRowsIter<'a> {
    type Item = &'a ArcadeGameEntry;

    fn next(&mut self) -> Option<Self::Item> {
        let ordinal = self.ordinal;
        self.ordinal = self.ordinal.saturating_add(1);
        self.rows.get(ordinal)
    }
}

#[derive(Debug)]
struct NavPackRow {
    game: ArcadeGameEntry,
    launch_index: Option<usize>,
    launch_plan: OnceLock<Option<StructuredLaunchPlan>>,
}

struct NavPackSystemRows {
    pack: Arc<crate::navpack::MappedNavPack>,
    system_id: Arc<str>,
    count: usize,
    row_pages: Vec<OnceLock<Box<[OnceLock<NavPackRow>]>>>,
    metadata_pages: Vec<OnceLock<Box<[OnceLock<ArcadeGameMetadataKey>]>>>,
    preview_ordinals: OnceLock<Vec<usize>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct NavPackCollectionOpenTiming {
    pub file_open_us: u64,
    pub mmap_us: u64,
    pub header_validation_us: u64,
    pub first_viewport_us: u64,
    pub total_us: u64,
}

impl std::fmt::Debug for NavPackSystemRows {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NavPackSystemRows")
            .field("system_id", &self.system_id)
            .field("count", &self.count)
            .field("resident_rows", &self.resident_rows())
            .finish()
    }
}

impl NavPackSystemRows {
    const FIRST_VIEWPORT_ROWS: usize = 10;
    const PAGE_ROWS: usize = 64;

    fn open(
        path: &Path,
        expected_bytes: u64,
        expected_system_id: &str,
        expected_generation: u64,
        expected_count: usize,
    ) -> Result<(Self, NavPackCollectionOpenTiming), String> {
        let total_started = std::time::Instant::now();
        let (pack, open) = crate::navpack::MappedNavPack::open(
            path,
            expected_bytes,
            expected_system_id,
            expected_generation,
            expected_count,
        )?;
        let page_count = expected_count.div_ceil(Self::PAGE_ROWS);
        let collection = Self {
            pack: Arc::new(pack),
            system_id: Arc::from(expected_system_id),
            count: expected_count,
            row_pages: std::iter::repeat_with(OnceLock::new)
                .take(page_count)
                .collect(),
            metadata_pages: std::iter::repeat_with(OnceLock::new)
                .take(page_count)
                .collect(),
            preview_ordinals: OnceLock::new(),
        };
        let viewport_started = std::time::Instant::now();
        for ordinal in 0..expected_count.min(Self::FIRST_VIEWPORT_ROWS) {
            collection.materialize(ordinal)?;
        }
        Ok((
            collection,
            NavPackCollectionOpenTiming {
                file_open_us: open.file_open_us,
                mmap_us: open.mmap_us,
                header_validation_us: open.header_validation_us,
                first_viewport_us: elapsed_micros(viewport_started),
                total_us: elapsed_micros(total_started),
            },
        ))
    }

    fn len(&self) -> usize {
        self.count
    }

    fn get(&self, ordinal: usize) -> Option<&ArcadeGameEntry> {
        self.materialize(ordinal).ok().map(|row| &row.game)
    }

    fn metadata_at(&self, ordinal: usize) -> Option<&ArcadeGameMetadataKey> {
        let slot = self.metadata_slot(ordinal)?;
        if let Some(metadata) = slot.get() {
            return Some(metadata);
        }
        let cold = self.pack.metadata(ordinal).ok()?;
        let _ = slot.set(ArcadeGameMetadataKey {
            year: cold.year,
            manufacturer: cold.manufacturer.to_string(),
            category: cold.category.to_string(),
            players: cold.players,
            control: cold.control.to_string(),
        });
        slot.get()
    }

    fn launch_plan_for_ref(&self, launch_ref: &str) -> Option<&StructuredLaunchPlan> {
        for page in self.row_pages.iter().filter_map(OnceLock::get) {
            for row in page.iter().filter_map(OnceLock::get) {
                if row.game.mra_path.as_ref() == launch_ref {
                    return self.launch_plan(row);
                }
            }
        }
        let ordinal = (0..self.len()).find(|ordinal| {
            self.pack
                .row(*ordinal)
                .is_ok_and(|row| row.launch_ref == launch_ref)
        })?;
        self.launch_plan(self.materialize(ordinal).ok()?)
    }

    fn preview_ordinals(&self) -> &[usize] {
        self.preview_ordinals.get_or_init(|| {
            (0..self.len())
                .filter(|ordinal| self.pack.row(*ordinal).is_ok_and(|row| row.has_preview))
                .collect()
        })
    }

    fn materialize(&self, ordinal: usize) -> Result<&NavPackRow, String> {
        let slot = self
            .row_slot(ordinal)
            .ok_or_else(|| "NavPack row ordinal is out of bounds".to_string())?;
        if let Some(row) = slot.get() {
            return Ok(row);
        }
        let hot = self.pack.row(ordinal)?;
        let title = Arc::<str>::from(hot.title);
        let launch_ref = Arc::<str>::from(hot.launch_ref);
        let preview_asset_key = if hot.preview_asset_key == hot.title {
            Arc::clone(&title)
        } else {
            Arc::from(hot.preview_asset_key)
        };
        let row = NavPackRow {
            game: ArcadeGameEntry {
                title,
                mra_path: launch_ref,
                preview_archive_path: Arc::from(hot.preview_archive_path),
                preview_asset_key,
                has_preview: hot.has_preview,
                system_id: Arc::clone(&self.system_id),
                year: None,
                manufacturer: Arc::from(""),
                category: Arc::from(""),
                players: None,
                control: Arc::from(""),
                is_new: hot.is_new,
            },
            launch_index: hot.launch_index,
            launch_plan: OnceLock::new(),
        };
        let _ = slot.set(row);
        slot.get()
            .ok_or_else(|| "NavPack row publication failed".to_string())
    }

    fn row_slot(&self, ordinal: usize) -> Option<&OnceLock<NavPackRow>> {
        let page_index = ordinal / Self::PAGE_ROWS;
        let in_page = ordinal % Self::PAGE_ROWS;
        let page = self.row_pages.get(page_index)?.get_or_init(|| {
            let count = self
                .count
                .saturating_sub(page_index * Self::PAGE_ROWS)
                .min(Self::PAGE_ROWS);
            std::iter::repeat_with(OnceLock::new)
                .take(count)
                .collect::<Vec<_>>()
                .into_boxed_slice()
        });
        page.get(in_page)
    }

    fn metadata_slot(&self, ordinal: usize) -> Option<&OnceLock<ArcadeGameMetadataKey>> {
        let page_index = ordinal / Self::PAGE_ROWS;
        let in_page = ordinal % Self::PAGE_ROWS;
        let page = self.metadata_pages.get(page_index)?.get_or_init(|| {
            let count = self
                .count
                .saturating_sub(page_index * Self::PAGE_ROWS)
                .min(Self::PAGE_ROWS);
            std::iter::repeat_with(OnceLock::new)
                .take(count)
                .collect::<Vec<_>>()
                .into_boxed_slice()
        });
        page.get(in_page)
    }

    fn launch_plan<'a>(&self, row: &'a NavPackRow) -> Option<&'a StructuredLaunchPlan> {
        row.launch_plan
            .get_or_init(|| {
                let plan = self.pack.launch(row.launch_index?).ok()?;
                Some(StructuredLaunchPlan {
                    launch_ref: if plan.launch_ref == row.game.mra_path.as_ref() {
                        Arc::clone(&row.game.mra_path)
                    } else {
                        Arc::from(plan.launch_ref)
                    },
                    title: if plan.title == row.game.title.as_ref() {
                        Arc::clone(&row.game.title)
                    } else {
                        Arc::from(plan.title)
                    },
                    system_id: if plan.system_id == self.system_id.as_ref() {
                        Arc::clone(&self.system_id)
                    } else {
                        Arc::from(plan.system_id)
                    },
                    core_path: Arc::from(plan.core_path),
                    payload_path: Arc::from(plan.payload_path),
                    mount_kind: Arc::from(plan.mount_kind),
                    mount_index: plan.mount_index,
                    delay_secs: plan.delay_secs,
                })
            })
            .as_ref()
    }

    fn resident_rows(&self) -> usize {
        self.row_pages
            .iter()
            .filter_map(OnceLock::get)
            .flat_map(|page| page.iter())
            .filter(|row| row.get().is_some())
            .count()
    }
}

fn elapsed_micros(started: std::time::Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

impl SystemCollection {
    pub fn new(
        system_id: impl Into<Arc<str>>,
        mut games: Vec<ArcadeGameEntry>,
        launch_plans: Vec<StructuredLaunchPlan>,
        platform_kind: PlatformKind,
    ) -> Self {
        let cold_metadata = games
            .iter_mut()
            .map(|game| {
                let metadata = game.metadata_key();
                game.year = None;
                game.manufacturer = Arc::from("");
                game.category = Arc::from("");
                game.players = None;
                game.control = Arc::from("");
                metadata
            })
            .collect();
        Self::new_with_metadata(system_id, games, cold_metadata, launch_plans, platform_kind)
    }

    pub fn new_with_metadata(
        system_id: impl Into<Arc<str>>,
        games: Vec<ArcadeGameEntry>,
        cold_metadata: Vec<ArcadeGameMetadataKey>,
        launch_plans: Vec<StructuredLaunchPlan>,
        platform_kind: PlatformKind,
    ) -> Self {
        let system_id = system_id.into();
        let mut launch_plans = launch_plans;
        debug_assert!(
            games
                .iter()
                .all(|game| game.system_id.as_ref() == system_id.as_ref())
        );
        debug_assert_eq!(games.len(), cold_metadata.len());
        let platform_kinds = Arc::new(HashMap::from([(system_id.to_string(), platform_kind)]));
        let preview_games_by_system =
            build_system_collection_preview_indexes(&games, &platform_kinds);
        launch_plans.sort_unstable_by(|left, right| left.launch_ref.cmp(&right.launch_ref));
        Self {
            system_id,
            games: SystemCollectionRows::Owned(Arc::new(games)),
            cold_metadata: Arc::new(cold_metadata),
            platform_kinds,
            preview_games_by_system: Arc::new(preview_games_by_system),
            launch_plans: Arc::new(launch_plans),
            rich_indexes: Arc::new(OnceLock::new()),
        }
    }

    fn all_launch_plans(&self) -> Vec<StructuredLaunchPlan> {
        match &self.games {
            SystemCollectionRows::Owned(_) => self.launch_plans.as_ref().clone(),
            SystemCollectionRows::NavPack(games) => (0..games.len())
                .filter_map(|ordinal| {
                    let row = games.materialize(ordinal).ok()?;
                    games.launch_plan(row).cloned()
                })
                .collect(),
        }
    }

    /// Opens a complete immutable collection from a checked, generation-bound NavPack mapping.
    ///
    /// Entry validates only the fixed header and materializes the first visible viewport. Every
    /// remaining ordinal stays synchronously addressable through the retained mapping.
    pub fn open_navpack(
        system_id: impl Into<Arc<str>>,
        navpack_path: &Path,
        navpack_bytes: u64,
        generation: u64,
        game_count: usize,
        platform_kind: PlatformKind,
    ) -> Result<(Self, NavPackCollectionOpenTiming), String> {
        let system_id = system_id.into();
        let (games, timing) = NavPackSystemRows::open(
            navpack_path,
            navpack_bytes,
            &system_id,
            generation,
            game_count,
        )?;
        let games = Arc::new(games);
        let platform_kinds = Arc::new(HashMap::from([(system_id.to_string(), platform_kind)]));
        Ok((
            Self {
                system_id,
                games: SystemCollectionRows::NavPack(games),
                cold_metadata: Arc::new(Vec::new()),
                platform_kinds,
                preview_games_by_system: Arc::new(HashMap::new()),
                launch_plans: Arc::new(Vec::new()),
                rich_indexes: Arc::new(OnceLock::new()),
            },
            timing,
        ))
    }

    pub fn game_at(&self, ordinal: usize) -> Option<&ArcadeGameEntry> {
        self.games.get(ordinal)
    }

    pub fn system_id(&self) -> &str {
        &self.system_id
    }

    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    fn filtered_game_indexes(&self, filter: &ArcadeFilter) -> &[usize] {
        filter_key(&self.system_id, filter)
            .and_then(|key| self.rich_indexes().games_by_filter.get(&key))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn preview_game_indexes(&self) -> &[usize] {
        match &self.games {
            SystemCollectionRows::NavPack(games) => return games.preview_ordinals(),
            SystemCollectionRows::Owned(_) => {}
        }
        self.preview_games_by_system
            .get(self.system_id.as_ref())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn filter_options(&self) -> &ArcadeSystemFilterOptions {
        static EMPTY: OnceLock<ArcadeSystemFilterOptions> = OnceLock::new();
        self.rich_indexes()
            .filter_options_by_system
            .get(self.system_id.as_ref())
            .unwrap_or_else(|| EMPTY.get_or_init(ArcadeSystemFilterOptions::default))
    }

    fn try_search_keys(&self) -> Option<&[ArcadeSearchKey]> {
        self.rich_indexes
            .get()
            .map(|indexes| indexes.search_keys.as_slice())
    }

    fn search_keys(&self) -> &[ArcadeSearchKey] {
        &self.rich_indexes().search_keys
    }

    fn try_autocomplete(&self) -> Option<&ArcadeAutocompleteIndex> {
        self.rich_indexes.get().map(|indexes| &indexes.autocomplete)
    }

    fn autocomplete(&self) -> &ArcadeAutocompleteIndex {
        &self.rich_indexes().autocomplete
    }

    fn metadata_at(&self, index: usize) -> Option<&ArcadeGameMetadataKey> {
        match &self.games {
            SystemCollectionRows::Owned(_) => self.cold_metadata.get(index),
            SystemCollectionRows::NavPack(games) => games.metadata_at(index),
        }
    }

    fn launch_plan_for_ref(&self, launch_ref: &str) -> Option<&StructuredLaunchPlan> {
        match &self.games {
            SystemCollectionRows::Owned(_) => self
                .launch_plans
                .binary_search_by(|plan| plan.launch_ref.as_ref().cmp(launch_ref))
                .ok()
                .and_then(|index| self.launch_plans.get(index)),
            SystemCollectionRows::NavPack(games) => games.launch_plan_for_ref(launch_ref),
        }
    }

    fn rich_indexes(&self) -> &ArcadeCatalogIndexes {
        self.rich_indexes.get_or_init(|| {
            let games = (0..self.games.len())
                .filter_map(|ordinal| {
                    let hot = self.games.get(ordinal)?;
                    let metadata = self.metadata_at(ordinal)?;
                    let mut game = hot.clone();
                    game.year = metadata.year;
                    game.manufacturer = Arc::from(metadata.manufacturer.as_str());
                    game.category = Arc::from(metadata.category.as_str());
                    game.players = metadata.players;
                    game.control = Arc::from(metadata.control.as_str());
                    Some(game)
                })
                .collect::<Vec<_>>();
            build_arcade_catalog_indexes(
                &games,
                &self.platform_kinds,
                self.all_launch_plans(),
                CatalogIndexMode::Eager,
                None,
            )
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArcadeTextIndexBuildTiming {
    pub built: bool,
    pub search_keys_us: u64,
    pub autocomplete_us: u64,
    pub total_us: u64,
}

/// O(1) background build handle detached from an [`ArcadeCatalog`].
///
/// It shares immutable game rows and the catalog's lazy index slot so the
/// catalog itself can be moved to the launcher before text indexing begins.
pub struct ArcadeTextIndexBuildJob {
    games: Arc<Vec<ArcadeGameEntry>>,
    platform_kinds: Arc<HashMap<String, PlatformKind>>,
    lazy_text_indexes: Arc<OnceLock<ArcadeTextIndexes>>,
}

impl ArcadeTextIndexBuildJob {
    pub fn text_index_token(&self) -> usize {
        Arc::as_ptr(&self.lazy_text_indexes) as usize
    }

    pub fn build_with_timing(self) -> ArcadeTextIndexBuildTiming {
        self.build_with_timing_while(|| true).unwrap_or_default()
    }

    /// Build deferred indexes while the caller still owns the runtime job.
    ///
    /// The predicate is sampled at bounded cooperative checkpoints.  A
    /// disconnected launcher can therefore stop an obsolete generation rather
    /// than continuing to consume an A9 after its result can no longer be
    /// published.
    pub fn build_with_timing_while<F>(self, keep_running: F) -> Option<ArcadeTextIndexBuildTiming>
    where
        F: FnMut() -> bool,
    {
        if self.lazy_text_indexes.get().is_some() {
            return Some(ArcadeTextIndexBuildTiming::default());
        }
        let (indexes, mut timing) = build_arcade_text_indexes_with_timing_while(
            &self.games,
            &self.platform_kinds,
            TextIndexBuildPacing::Interactive,
            keep_running,
        )?;
        timing.built = self.lazy_text_indexes.set(indexes).is_ok();
        if timing.built {
            Some(timing)
        } else {
            Some(ArcadeTextIndexBuildTiming::default())
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ArcadeGameView<'a> {
    Contiguous(&'a [ArcadeGameEntry]),
    Indexed {
        games: &'a [ArcadeGameEntry],
        indexes: &'a [usize],
    },
    Collection(&'a SystemCollection),
    CollectionIndexed {
        collection: &'a SystemCollection,
        indexes: &'a [usize],
    },
}

impl<'a> ArcadeGameView<'a> {
    pub fn empty() -> Self {
        Self::Contiguous(&[])
    }

    pub fn contiguous(games: &'a [ArcadeGameEntry]) -> Self {
        Self::Contiguous(games)
    }

    pub fn indexed(games: &'a [ArcadeGameEntry], indexes: &'a [usize]) -> Self {
        Self::Indexed { games, indexes }
    }

    fn collection(collection: &'a SystemCollection) -> Self {
        Self::Collection(collection)
    }

    fn collection_indexed(collection: &'a SystemCollection, indexes: &'a [usize]) -> Self {
        Self::CollectionIndexed {
            collection,
            indexes,
        }
    }

    pub fn len(self) -> usize {
        match self {
            Self::Contiguous(games) => games.len(),
            Self::Indexed { indexes, .. } => indexes.len(),
            Self::Collection(collection) => collection.len(),
            Self::CollectionIndexed { indexes, .. } => indexes.len(),
        }
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn get(self, index: usize) -> Option<&'a ArcadeGameEntry> {
        match self {
            Self::Contiguous(games) => games.get(index),
            Self::Indexed { games, indexes } => indexes
                .get(index)
                .and_then(|game_index| games.get(*game_index)),
            Self::Collection(collection) => collection.games.get(index),
            Self::CollectionIndexed {
                collection,
                indexes,
            } => indexes
                .get(index)
                .and_then(|game_index| collection.games.get(*game_index)),
        }
    }

    pub fn iter(self) -> ArcadeGameViewIter<'a> {
        ArcadeGameViewIter {
            view: self,
            index: 0,
        }
    }
}

pub struct ArcadeGameViewIter<'a> {
    view: ArcadeGameView<'a>,
    index: usize,
}

impl<'a> Iterator for ArcadeGameViewIter<'a> {
    type Item = &'a ArcadeGameEntry;

    fn next(&mut self) -> Option<Self::Item> {
        let game = self.view.get(self.index);
        self.index += 1;
        game
    }
}

#[derive(Clone, Debug, Default)]
struct ArcadeSystemFilterOptions {
    categories: Vec<ArcadeFilterOption>,
    decades: Vec<ArcadeFilterOption>,
    manufacturers: Vec<ArcadeFilterOption>,
    players: Vec<ArcadeFilterOption>,
    controls: Vec<ArcadeFilterOption>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct ArcadeFilterKey {
    system_id: Arc<str>,
    kind: ArcadeFilterKindKey,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum ArcadeFilterKindKey {
    Category(Arc<str>),
    Decade(u16),
    Manufacturer(Arc<str>),
    Players(u8),
    Control(Arc<str>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuredLaunchPlan {
    pub launch_ref: Arc<str>,
    pub title: Arc<str>,
    pub system_id: Arc<str>,
    pub core_path: Arc<str>,
    pub payload_path: Arc<str>,
    pub mount_kind: Arc<str>,
    pub mount_index: u8,
    pub delay_secs: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedLaunchSelection {
    pub collection_id: PreparedCollectionId,
    pub launch_ref: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchTarget {
    Path(Arc<str>),
    Structured(StructuredLaunchPlan),
    Prepared(PreparedLaunchSelection),
    MissingStructured(Arc<str>),
}

impl ArcadeCatalog {
    pub fn new(root: PathBuf, games: Vec<ArcadeGameEntry>, systems: Vec<GameSystemEntry>) -> Self {
        Self::new_with_launch_plans(root, games, systems, Vec::new())
    }

    pub fn new_with_launch_plans(
        root: PathBuf,
        games: Vec<ArcadeGameEntry>,
        systems: Vec<GameSystemEntry>,
        launch_plans: Vec<StructuredLaunchPlan>,
    ) -> Self {
        Self::new_with_launch_plans_and_index_mode(
            root,
            games,
            systems,
            launch_plans,
            CatalogIndexMode::Eager,
        )
    }

    pub fn new_with_deferred_text_indexes(
        root: PathBuf,
        games: Vec<ArcadeGameEntry>,
        systems: Vec<GameSystemEntry>,
        launch_plans: Vec<StructuredLaunchPlan>,
    ) -> Self {
        Self::new_with_launch_plans_and_index_mode(
            root,
            games,
            systems,
            launch_plans,
            CatalogIndexMode::DeferredText,
        )
    }

    pub fn new_with_deferred_text_indexes_and_platform_kinds(
        root: PathBuf,
        games: Vec<ArcadeGameEntry>,
        systems: Vec<GameSystemEntry>,
        launch_plans: Vec<StructuredLaunchPlan>,
        platform_kinds: HashMap<String, PlatformKind>,
    ) -> Self {
        Self::new_with_deferred_text_indexes_and_platform_kinds_with_progress(
            root,
            games,
            systems,
            launch_plans,
            platform_kinds,
            None,
        )
    }

    pub(crate) fn new_with_deferred_text_indexes_and_platform_kinds_with_progress(
        root: PathBuf,
        games: Vec<ArcadeGameEntry>,
        systems: Vec<GameSystemEntry>,
        launch_plans: Vec<StructuredLaunchPlan>,
        platform_kinds: HashMap<String, PlatformKind>,
        index_progress: Option<&mut dyn FnMut(usize, usize)>,
    ) -> Self {
        Self::new_with_launch_plans_and_index_mode_and_platform_kinds(
            root,
            games,
            systems,
            launch_plans,
            CatalogIndexMode::DeferredText,
            platform_kinds,
            index_progress,
        )
    }

    fn new_with_launch_plans_and_index_mode(
        root: PathBuf,
        games: Vec<ArcadeGameEntry>,
        systems: Vec<GameSystemEntry>,
        launch_plans: Vec<StructuredLaunchPlan>,
        index_mode: CatalogIndexMode,
    ) -> Self {
        let platform_kinds = inferred_platform_kinds(&systems);
        Self::new_with_launch_plans_and_index_mode_and_platform_kinds(
            root,
            games,
            systems,
            launch_plans,
            index_mode,
            platform_kinds,
            None,
        )
    }

    fn new_with_launch_plans_and_index_mode_and_platform_kinds(
        root: PathBuf,
        games: Vec<ArcadeGameEntry>,
        mut systems: Vec<GameSystemEntry>,
        launch_plans: Vec<StructuredLaunchPlan>,
        index_mode: CatalogIndexMode,
        mut platform_kinds: HashMap<String, PlatformKind>,
        index_progress: Option<&mut dyn FnMut(usize, usize)>,
    ) -> Self {
        sort_systems_by_title(&mut systems);
        fill_missing_platform_kinds(&systems, &mut platform_kinds);
        let platform_kinds = Arc::new(platform_kinds);
        let indexes = build_arcade_catalog_indexes(
            &games,
            &platform_kinds,
            launch_plans,
            index_mode,
            index_progress,
        );
        Self {
            root,
            games: Arc::new(games),
            systems,
            platform_kinds,
            games_by_system: Arc::new(indexes.games_by_system),
            games_by_filter: Arc::new(indexes.games_by_filter),
            filter_options_by_system: Arc::new(indexes.filter_options_by_system),
            preview_games_by_system: Arc::new(indexes.preview_games_by_system),
            launch_plans_by_ref: Arc::new(indexes.launch_plans_by_ref),
            projection_stats_by_system: Arc::new(HashMap::new()),
            search_keys: Arc::new(indexes.search_keys),
            autocomplete: Arc::new(indexes.autocomplete),
            lazy_text_indexes: Arc::new(OnceLock::new()),
            system_collections: Arc::new(HashMap::new()),
        }
    }

    pub fn from_navigation_projection(
        root: impl Into<PathBuf>,
        projection: CatalogNavigationProjection,
    ) -> Self {
        Self::from_navigation_projection_with_index_mode(
            root,
            projection,
            CatalogIndexMode::DeferredText,
        )
    }

    fn from_navigation_projection_with_index_mode(
        root: impl Into<PathBuf>,
        projection: CatalogNavigationProjection,
        index_mode: CatalogIndexMode,
    ) -> Self {
        let platform_kinds = projection
            .systems
            .iter()
            .map(|system| (system.id.clone(), system.platform_kind))
            .collect();
        let games: Vec<ArcadeGameEntry> = projection
            .games
            .into_iter()
            .map(ArcadeGameEntry::from)
            .collect();
        let systems = projection
            .systems
            .into_iter()
            .map(GameSystemEntry::from)
            .collect();
        let launch_plans = projection
            .launch_plans
            .into_iter()
            .map(StructuredLaunchPlan::from)
            .collect();
        Self::new_with_launch_plans_and_index_mode_and_platform_kinds(
            root.into(),
            games,
            systems,
            launch_plans,
            index_mode,
            platform_kinds,
            None,
        )
    }

    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    pub(crate) fn with_projection_stats(
        mut self,
        stats: HashMap<String, SystemProjectionStats>,
    ) -> Self {
        Arc::make_mut(&mut self.projection_stats_by_system).extend(stats);
        self
    }

    #[cfg(feature = "builder")]
    pub(crate) fn system_projection_stats(&self, system_id: &str) -> Option<SystemProjectionStats> {
        self.projection_stats_by_system.get(system_id).copied()
    }

    pub fn platform_kind(&self, system_id: &str) -> PlatformKind {
        self.platform_kinds
            .get(system_id)
            .copied()
            .unwrap_or_else(|| PlatformKind::inferred_for_system_id(system_id))
    }

    pub fn title_for_path(&self, mra_path: &str) -> &str {
        self.system_collections
            .values()
            .find_map(|collection| {
                collection
                    .games
                    .iter()
                    .find(|game| game.mra_path.as_ref() == mra_path)
            })
            .or_else(|| self.games.iter().find(|g| g.mra_path.as_ref() == mra_path))
            .map(|g| g.title.as_ref())
            .unwrap_or("Game")
    }

    pub fn game_for_launch_ref(&self, launch_ref: &str) -> Option<&ArcadeGameEntry> {
        self.system_collections
            .values()
            .find_map(|collection| {
                collection
                    .games
                    .iter()
                    .find(|game| game.mra_path.as_ref() == launch_ref)
            })
            .or_else(|| {
                self.games
                    .iter()
                    .find(|game| game.mra_path.as_ref() == launch_ref)
            })
    }

    pub fn user_game_identity_for_ref(
        &self,
        launch_ref: &str,
    ) -> Option<crate::user_state::UserGameIdentity> {
        let game = self.game_for_launch_ref(launch_ref)?;
        let payload_path = self
            .structured_launch_plan_for_ref(launch_ref)
            .map(|plan| plan.payload_path.to_string())
            .unwrap_or_else(|| launch_ref.to_string());
        Some(crate::user_state::UserGameIdentity {
            system_id: game.system_id.to_string(),
            stable_key: game.stable_key(),
            title: game.title.to_string(),
            launch_ref: launch_ref.to_string(),
            payload_path,
        })
    }

    pub fn launch_target_for_ref(&self, launch_ref: &str) -> LaunchTarget {
        if launch_ref.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX)
            || launch_ref == AMIGAVISION_LAUNCHER_REF
        {
            return LaunchTarget::Prepared(PreparedLaunchSelection {
                collection_id: PreparedCollectionId::AmigaVision,
                launch_ref: Arc::from(launch_ref),
            });
        }
        self.structured_launch_plan_for_ref(launch_ref)
            .cloned()
            .map(LaunchTarget::Structured)
            .unwrap_or_else(|| {
                if launch_ref.starts_with("magik-plan:") {
                    LaunchTarget::MissingStructured(Arc::from(launch_ref))
                } else {
                    LaunchTarget::Path(Arc::from(launch_ref))
                }
            })
    }

    pub(crate) fn structured_launch_plan_for_ref(
        &self,
        launch_ref: &str,
    ) -> Option<&StructuredLaunchPlan> {
        self.system_collections
            .values()
            .find_map(|collection| collection.launch_plan_for_ref(launch_ref))
            .or_else(|| self.launch_plans_by_ref.get(launch_ref))
    }

    pub fn system_games(&self, system_id: &str) -> Vec<ArcadeGameEntry> {
        self.system_game_view(system_id).iter().cloned().collect()
    }

    #[cfg(feature = "builder")]
    pub(crate) fn isolated_system_catalog(&self, system_id: &str) -> Self {
        let games = self.system_games(system_id);
        let retained_refs = games
            .iter()
            .map(|game| game.mra_path.clone())
            .collect::<BTreeSet<_>>();
        let launch_plans = self
            .launch_plans_by_ref
            .values()
            .filter(|plan| retained_refs.contains(&plan.launch_ref))
            .cloned()
            .collect::<Vec<_>>();
        let systems = self
            .systems
            .iter()
            .filter(|system| system.id == system_id)
            .cloned()
            .collect::<Vec<_>>();
        let platform_kinds = self
            .platform_kinds
            .get(system_id)
            .copied()
            .map(|kind| HashMap::from([(system_id.to_string(), kind)]))
            .unwrap_or_default();
        Self::new_with_deferred_text_indexes_and_platform_kinds(
            self.root.clone(),
            games,
            systems,
            launch_plans,
            platform_kinds,
        )
    }

    pub fn replacing_system_games(
        &self,
        system_id: &str,
        replacement: Vec<ArcadeGameEntry>,
        replacement_plans: Vec<StructuredLaunchPlan>,
    ) -> Self {
        self.with_system_collection(Arc::new(SystemCollection::new(
            system_id,
            replacement,
            replacement_plans,
            self.platform_kind(system_id),
        )))
    }

    pub fn with_system_collection(&self, collection: Arc<SystemCollection>) -> Self {
        let system_id = collection.system_id().to_string();
        self.with_system_collection_for_id(system_id, collection)
    }

    /// Publishes a collection under a launcher collection alias.
    ///
    /// The Home Arcade tile uses `menu:arcade`, while its immutable artifact identity remains
    /// `arcade`. Keeping the alias at the catalog boundary preserves row and launch identities.
    pub fn with_system_collection_for_id(
        &self,
        collection_id: impl Into<String>,
        collection: Arc<SystemCollection>,
    ) -> Self {
        let mut catalog = self.clone();
        let collection_id = collection_id.into();
        if let Some(system) = catalog
            .systems
            .iter_mut()
            .find(|system| system.id == collection_id)
        {
            system.count = collection.len();
        }
        Arc::make_mut(&mut catalog.system_collections).insert(collection_id, collection);
        catalog
    }

    /// Adds a zero-game system shell so progressive launcher taxonomy can
    /// publish the real destination before its shard is ready.
    pub fn with_system_placeholder(&self, system_id: &str) -> Self {
        if self.systems.iter().any(|system| system.id == system_id) {
            return self.clone();
        }
        let mut catalog = self.clone();
        catalog.systems.push(GameSystemEntry {
            id: system_id.to_string(),
            title: crate::catalog_classify::system_definition(system_id)
                .map(|definition| definition.title.clone())
                .unwrap_or_else(|| system_id.to_ascii_uppercase()),
            count: 0,
        });
        sort_systems_by_title(&mut catalog.systems);
        Arc::make_mut(&mut catalog.platform_kinds).insert(
            system_id.to_string(),
            crate::catalog_classify::platform_kind_for_system(system_id),
        );
        catalog
    }

    /// Removes zero-game system shells after an authoritative catalog replaces
    /// the progressive discovery view.
    pub fn without_empty_system_placeholders(&self) -> Self {
        let mut catalog = self.clone();
        catalog
            .systems
            .retain(|system| system.count > 0 || self.system_game_count(&system.id) > 0);
        if catalog.systems.len() == self.systems.len() {
            return self.clone();
        }
        let retained_system_ids = catalog
            .systems
            .iter()
            .map(|system| system.id.as_str())
            .collect::<HashSet<_>>();
        Arc::make_mut(&mut catalog.platform_kinds)
            .retain(|system_id, _| retained_system_ids.contains(system_id.as_str()));
        Arc::make_mut(&mut catalog.projection_stats_by_system)
            .retain(|system_id, _| retained_system_ids.contains(system_id.as_str()));
        Arc::make_mut(&mut catalog.system_collections)
            .retain(|system_id, _| retained_system_ids.contains(system_id.as_str()));
        catalog
    }

    pub fn system_game_count(&self, system_id: &str) -> usize {
        self.system_collection(system_id).map_or_else(
            || self.system_game_indexes(system_id).len(),
            |value| value.len(),
        )
    }

    pub fn system_game_at(&self, system_id: &str, index: usize) -> Option<&ArcadeGameEntry> {
        self.system_game_view(system_id).get(index)
    }

    /// Returns rich metadata for a system row without placing it in the hot navigation row.
    pub fn system_game_metadata_at(
        &self,
        system_id: &str,
        index: usize,
    ) -> Option<ArcadeGameMetadataKey> {
        self.system_collection(system_id).map_or_else(
            || {
                self.system_game_at(system_id, index)
                    .map(ArcadeGameEntry::metadata_key)
            },
            |collection| collection.metadata_at(index).cloned(),
        )
    }

    pub fn system_game_view(&self, system_id: &str) -> ArcadeGameView<'_> {
        self.system_collection(system_id).map_or_else(
            || ArcadeGameView::indexed(&self.games, self.system_game_indexes(system_id)),
            ArcadeGameView::collection,
        )
    }

    pub fn indexed_system_game_view<'a>(
        &'a self,
        system_id: &str,
        indexes: &'a [usize],
    ) -> ArcadeGameView<'a> {
        self.system_collection(system_id).map_or_else(
            || ArcadeGameView::indexed(&self.games, indexes),
            |collection| ArcadeGameView::collection_indexed(collection, indexes),
        )
    }

    pub fn search_source_system_ids(&self, collection_id: &str) -> Vec<String> {
        let mut seen = HashSet::new();
        self.system_game_view(collection_id)
            .iter()
            .filter_map(|game| {
                let system_id = game.system_id.to_string();
                seen.insert(system_id.clone()).then_some(system_id)
            })
            .collect()
    }

    pub fn resolve_system_game_ordinal(&self, system_id: &str, ordinal: usize) -> Option<usize> {
        self.system_collection(system_id).map_or_else(
            || self.system_game_indexes(system_id).get(ordinal).copied(),
            |collection| (ordinal < collection.len()).then_some(ordinal),
        )
    }

    pub fn collection_game_index_set(&self, collection_id: &str) -> HashSet<usize> {
        self.system_collection(collection_id).map_or_else(
            || {
                self.system_game_indexes(collection_id)
                    .iter()
                    .copied()
                    .collect()
            },
            |collection| (0..collection.len()).collect(),
        )
    }

    pub fn search_game_indexes(&self, system_id: &str, query: &str) -> Vec<usize> {
        let needle = search_match_key(query);
        let compact_needle = compact_search_match_key(query);
        let tokens = search_query_tokens(&needle);
        if needle.is_empty() && compact_needle.is_empty() {
            return self.system_collection(system_id).map_or_else(
                || self.system_game_indexes(system_id).to_vec(),
                |collection| (0..collection.len()).collect(),
            );
        }
        let collection = self.system_collection(system_id);
        let indexes = collection.map_or_else(
            || self.system_game_indexes(system_id).to_vec(),
            |collection| (0..collection.len()).collect(),
        );
        let search_keys =
            collection.map_or_else(|| self.search_keys(), |value| value.search_keys());
        let mut scored: Vec<SearchMatch> = indexes
            .into_iter()
            .filter_map(|index| {
                let score = search_keys
                    .get(index)
                    .and_then(|key| key.score(&tokens, &compact_needle))?;
                Some(SearchMatch { index, score })
            })
            .collect();
        scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.index.cmp(&b.index)));
        scored.into_iter().map(|entry| entry.index).collect()
    }

    /// Searches only when the text indexes are already available.
    ///
    /// Unlike [`Self::search_game_indexes`], this never builds indexes on the
    /// caller and is therefore safe to use from the launcher event loop.
    pub fn try_search_game_indexes(&self, system_id: &str, query: &str) -> Option<Vec<usize>> {
        let needle = search_match_key(query);
        let compact_needle = compact_search_match_key(query);
        let tokens = search_query_tokens(&needle);
        if needle.is_empty() && compact_needle.is_empty() {
            return Some(self.system_collection(system_id).map_or_else(
                || self.system_game_indexes(system_id).to_vec(),
                |collection| (0..collection.len()).collect(),
            ));
        }
        let collection = self.system_collection(system_id);
        let search_keys = match collection {
            Some(collection) => collection.try_search_keys()?,
            None => self.try_search_keys()?,
        };
        let indexes = collection.map_or_else(
            || self.system_game_indexes(system_id).to_vec(),
            |collection| (0..collection.len()).collect(),
        );
        let mut scored: Vec<SearchMatch> = indexes
            .into_iter()
            .filter_map(|index| {
                let score = search_keys
                    .get(index)
                    .and_then(|key| key.score(&tokens, &compact_needle))?;
                Some(SearchMatch { index, score })
            })
            .collect();
        scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.index.cmp(&b.index)));
        Some(scored.into_iter().map(|entry| entry.index).collect())
    }

    pub fn autocomplete_search_word(&self, system_id: &str, query: &str) -> String {
        let fragment = current_search_word(query);
        self.system_collection(system_id)
            .map_or_else(
                || self.autocomplete(),
                |collection| collection.autocomplete(),
            )
            .suggest(system_id, fragment)
            .unwrap_or_default()
    }

    /// Returns an autocomplete suggestion only when its index is already
    /// available, without performing work on the caller.
    pub fn try_autocomplete_search_word(&self, system_id: &str, query: &str) -> Option<String> {
        let fragment = current_search_word(query);
        let autocomplete = match self.system_collection(system_id) {
            Some(collection) => collection.try_autocomplete()?,
            None => self.try_autocomplete()?,
        };
        Some(
            autocomplete
                .suggest(system_id, fragment)
                .unwrap_or_default(),
        )
    }

    fn try_search_keys(&self) -> Option<&[ArcadeSearchKey]> {
        if self.search_keys.len() == self.games.len() {
            Some(&self.search_keys)
        } else {
            self.lazy_text_indexes
                .get()
                .map(|indexes| indexes.search_keys.as_slice())
        }
    }

    fn try_autocomplete(&self) -> Option<&ArcadeAutocompleteIndex> {
        if self.autocomplete.is_empty() && !self.games.is_empty() {
            self.lazy_text_indexes
                .get()
                .map(|indexes| &indexes.autocomplete)
        } else {
            Some(&self.autocomplete)
        }
    }

    fn search_keys(&self) -> &[ArcadeSearchKey] {
        if self.search_keys.len() == self.games.len() {
            &self.search_keys
        } else {
            &self.lazy_text_indexes().search_keys
        }
    }

    fn autocomplete(&self) -> &ArcadeAutocompleteIndex {
        if self.autocomplete.is_empty() && !self.games.is_empty() {
            &self.lazy_text_indexes().autocomplete
        } else {
            &self.autocomplete
        }
    }

    fn lazy_text_indexes(&self) -> &ArcadeTextIndexes {
        self.lazy_text_indexes
            .get_or_init(|| build_arcade_text_indexes(&self.games, &self.platform_kinds))
    }

    pub fn text_indexes_ready(&self) -> bool {
        self.search_keys.len() == self.games.len()
            || !self.autocomplete.is_empty()
            || self.lazy_text_indexes.get().is_some()
    }

    /// Stable identity shared by catalog clones that publish the same lazy
    /// text-index build. Used to discard stale worker completion messages.
    pub fn text_index_token(&self) -> usize {
        Arc::as_ptr(&self.lazy_text_indexes) as usize
    }

    pub fn text_index_build_job(&self) -> Option<ArcadeTextIndexBuildJob> {
        (!self.text_indexes_ready()).then(|| ArcadeTextIndexBuildJob {
            games: Arc::clone(&self.games),
            platform_kinds: Arc::clone(&self.platform_kinds),
            lazy_text_indexes: Arc::clone(&self.lazy_text_indexes),
        })
    }

    pub fn ensure_text_indexes_ready(&self) -> bool {
        self.ensure_text_indexes_ready_with_timing().built
    }

    pub fn ensure_text_indexes_ready_with_timing(&self) -> ArcadeTextIndexBuildTiming {
        let was_ready = self.text_indexes_ready();
        if was_ready {
            return ArcadeTextIndexBuildTiming::default();
        }
        let (indexes, mut timing) = build_arcade_text_indexes_with_timing_while(
            &self.games,
            &self.platform_kinds,
            TextIndexBuildPacing::Unthrottled,
            || true,
        )
        .expect("unthrottled text-index build cannot be cancelled");
        timing.built = self.lazy_text_indexes.set(indexes).is_ok();
        if !timing.built {
            ArcadeTextIndexBuildTiming::default()
        } else {
            timing
        }
    }

    pub fn filtered_game_count(&self, system_id: &str, filter: &ArcadeFilter) -> usize {
        match filter {
            ArcadeFilter::All => self.system_game_count(system_id),
            ArcadeFilter::Search => self.system_game_count(system_id),
            _ => self.system_collection(system_id).map_or_else(
                || self.filtered_game_indexes(system_id, filter).len(),
                |collection| collection.filtered_game_indexes(filter).len(),
            ),
        }
    }

    pub fn filtered_game_at(
        &self,
        system_id: &str,
        filter: &ArcadeFilter,
        index: usize,
    ) -> Option<&ArcadeGameEntry> {
        match filter {
            ArcadeFilter::All => self.system_game_at(system_id, index),
            ArcadeFilter::Search => self.system_game_at(system_id, index),
            _ => self.system_collection(system_id).map_or_else(
                || {
                    self.filtered_game_indexes(system_id, filter)
                        .get(index)
                        .and_then(|game_index| self.games.get(*game_index))
                },
                |collection| {
                    collection
                        .filtered_game_indexes(filter)
                        .get(index)
                        .and_then(|game_index| collection.games.get(*game_index))
                },
            ),
        }
    }

    pub fn filtered_game_view(&self, system_id: &str, filter: &ArcadeFilter) -> ArcadeGameView<'_> {
        match filter {
            ArcadeFilter::All => self.system_game_view(system_id),
            ArcadeFilter::Search => self.system_game_view(system_id),
            _ => self.system_collection(system_id).map_or_else(
                || {
                    ArcadeGameView::indexed(
                        &self.games,
                        self.filtered_game_indexes(system_id, filter),
                    )
                },
                |collection| {
                    ArcadeGameView::collection_indexed(
                        collection,
                        collection.filtered_game_indexes(filter),
                    )
                },
            ),
        }
    }

    pub fn decade_options(&self, system_id: &str) -> Vec<ArcadeFilterOption> {
        self.filter_options(system_id).decades.clone()
    }

    pub fn category_options(&self, system_id: &str) -> Vec<ArcadeFilterOption> {
        self.filter_options(system_id).categories.clone()
    }

    pub fn category_option_count(&self, system_id: &str) -> usize {
        self.filter_options(system_id).categories.len()
    }

    pub fn decade_option_count(&self, system_id: &str) -> usize {
        self.filter_options(system_id).decades.len()
    }

    pub fn manufacturer_options(&self, system_id: &str) -> Vec<ArcadeFilterOption> {
        self.filter_options(system_id).manufacturers.clone()
    }

    pub fn manufacturer_option_count(&self, system_id: &str) -> usize {
        self.filter_options(system_id).manufacturers.len()
    }

    pub fn player_options(&self, system_id: &str) -> Vec<ArcadeFilterOption> {
        self.filter_options(system_id).players.clone()
    }

    pub fn player_option_count(&self, system_id: &str) -> usize {
        self.filter_options(system_id).players.len()
    }

    pub fn control_options(&self, system_id: &str) -> Vec<ArcadeFilterOption> {
        self.filter_options(system_id).controls.clone()
    }

    pub fn control_option_count(&self, system_id: &str) -> usize {
        self.filter_options(system_id).controls.len()
    }

    pub fn resolve_filter(&self, system_id: &str, filter: &ArcadeFilter) -> Option<ArcadeFilter> {
        match filter {
            ArcadeFilter::All => Some(ArcadeFilter::All),
            ArcadeFilter::Search => Some(ArcadeFilter::Search),
            ArcadeFilter::Category(category) => self
                .category_options(system_id)
                .iter()
                .find(|option| option.label == category.trim())
                .map(|option| ArcadeFilter::Category(option.label.clone())),
            ArcadeFilter::Decade(decade) => self
                .decade_options(system_id)
                .iter()
                .any(|option| option.label == format!("{decade}'s"))
                .then_some(ArcadeFilter::Decade(*decade)),
            ArcadeFilter::Manufacturer(manufacturer) => self
                .manufacturer_options(system_id)
                .iter()
                .find(|option| option.label == manufacturer.trim())
                .map(|option| ArcadeFilter::Manufacturer(option.label.clone())),
            ArcadeFilter::Players(players) => self
                .player_options(system_id)
                .iter()
                .any(|option| option.label == player_count_label(*players))
                .then_some(ArcadeFilter::Players(*players)),
            ArcadeFilter::Control(control) => {
                let control = canonical_control_label(control);
                self.control_options(system_id)
                    .iter()
                    .find(|option| option.label == control)
                    .map(|option| ArcadeFilter::Control(option.label.clone()))
            }
        }
    }

    pub fn filter_option_count_detail(&self, system_id: &str) -> String {
        format!(
            "collection={} games={} categories={} decades={} manufacturers={} players={} controls={}",
            system_id,
            self.system_game_count(system_id),
            self.category_option_count(system_id),
            self.decade_option_count(system_id),
            self.manufacturer_option_count(system_id),
            self.player_option_count(system_id),
            self.control_option_count(system_id)
        )
    }

    pub fn filter_option_mismatches(&self, other: &Self) -> Vec<String> {
        let collection_ids = self
            .systems
            .iter()
            .map(|system| system.id.as_str())
            .chain(other.systems.iter().map(|system| system.id.as_str()))
            .chain([MENU_ARCADE_SYSTEM_ID])
            .collect::<BTreeSet<_>>();
        collection_ids
            .into_iter()
            .filter(|system_id| {
                self.system_game_count(system_id) != other.system_game_count(system_id)
                    || self.decade_options(system_id) != other.decade_options(system_id)
                    || self.manufacturer_options(system_id) != other.manufacturer_options(system_id)
                    || self.player_options(system_id) != other.player_options(system_id)
                    || self.control_options(system_id) != other.control_options(system_id)
            })
            .map(|system_id| {
                format!(
                    "{} expected=({}) actual=({})",
                    system_id,
                    self.filter_option_count_detail(system_id),
                    other.filter_option_count_detail(system_id)
                )
            })
            .collect()
    }

    pub fn system_preview_games(&self, system_id: &str) -> Vec<ArcadeGameEntry> {
        self.system_collection(system_id).map_or_else(
            || {
                self.preview_game_indexes(system_id)
                    .iter()
                    .filter_map(|index| self.games.get(*index).cloned())
                    .collect()
            },
            |collection| {
                collection
                    .preview_game_indexes()
                    .iter()
                    .filter_map(|index| collection.games.get(*index).cloned())
                    .collect()
            },
        )
    }

    pub fn system_preview_game_count(&self, system_id: &str) -> usize {
        self.system_collection(system_id).map_or_else(
            || self.preview_game_indexes(system_id).len(),
            |collection| collection.preview_game_indexes().len(),
        )
    }

    pub fn system_preview_game_at(&self, system_id: &str, index: usize) -> Option<ArcadeGameEntry> {
        self.system_collection(system_id).map_or_else(
            || {
                self.preview_game_indexes(system_id)
                    .get(index)
                    .and_then(|game_index| self.games.get(*game_index))
                    .cloned()
            },
            |collection| {
                collection
                    .preview_game_indexes()
                    .get(index)
                    .and_then(|game_index| collection.games.get(*game_index))
                    .cloned()
            },
        )
    }

    fn preview_game_indexes(&self, system_id: &str) -> &[usize] {
        self.preview_games_by_system
            .get(system_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn filtered_game_indexes(&self, system_id: &str, filter: &ArcadeFilter) -> &[usize] {
        filter_key(system_id, filter)
            .and_then(|key| self.games_by_filter.get(&key))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn system_game_indexes(&self, system_id: &str) -> &[usize] {
        self.games_by_system
            .get(system_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn filter_options(&self, system_id: &str) -> &ArcadeSystemFilterOptions {
        static EMPTY: OnceLock<ArcadeSystemFilterOptions> = OnceLock::new();
        self.system_collection(system_id).map_or_else(
            || {
                self.filter_options_by_system
                    .get(system_id)
                    .unwrap_or_else(|| EMPTY.get_or_init(ArcadeSystemFilterOptions::default))
            },
            SystemCollection::filter_options,
        )
    }

    fn system_collection(&self, system_id: &str) -> Option<&SystemCollection> {
        self.system_collections.get(system_id).map(Arc::as_ref)
    }
}

#[derive(Debug)]
struct ArcadeCatalogIndexes {
    games_by_system: HashMap<String, Vec<usize>>,
    games_by_filter: HashMap<ArcadeFilterKey, Vec<usize>>,
    filter_options_by_system: HashMap<String, ArcadeSystemFilterOptions>,
    preview_games_by_system: HashMap<String, Vec<usize>>,
    launch_plans_by_ref: HashMap<Arc<str>, StructuredLaunchPlan>,
    search_keys: Vec<ArcadeSearchKey>,
    autocomplete: ArcadeAutocompleteIndex,
}

#[derive(Clone, Debug, Default)]
struct ArcadeTextIndexes {
    search_keys: Vec<ArcadeSearchKey>,
    autocomplete: ArcadeAutocompleteIndex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogIndexMode {
    Eager,
    DeferredText,
}

fn game_collection_ids<'a>(
    game: &'a ArcadeGameEntry,
    _platform_kinds: &HashMap<String, PlatformKind>,
) -> [Option<&'a str>; 2] {
    let system_id = game.system_id.as_ref();
    let mut ids = [Some(system_id), None];
    let belongs_to_arcade =
        crate::catalog_classify::system_definition(system_id).is_some_and(|definition| {
            definition.section == crate::catalog_classify::LauncherSection::Arcade
        });
    if belongs_to_arcade {
        ids[1] = Some(MENU_ARCADE_SYSTEM_ID);
    }
    ids
}

#[derive(Clone, Debug)]
struct ArcadeSearchKey {
    title: String,
    path: String,
    manufacturer: String,
    category: String,
    players: String,
    control: String,
    year: String,
    decade: String,
    compact_title: String,
    compact_path: String,
    compact_manufacturer: String,
    compact_category: String,
    compact_control: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SearchMatch {
    index: usize,
    score: u16,
}

fn build_arcade_catalog_indexes(
    games: &[ArcadeGameEntry],
    platform_kinds: &HashMap<String, PlatformKind>,
    launch_plans: Vec<StructuredLaunchPlan>,
    mode: CatalogIndexMode,
    mut index_progress: Option<&mut dyn FnMut(usize, usize)>,
) -> ArcadeCatalogIndexes {
    let mut games_by_system: HashMap<String, Vec<usize>> = HashMap::new();
    let mut games_by_filter: HashMap<ArcadeFilterKey, Vec<usize>> = HashMap::new();
    let mut filter_counts_by_system = HashMap::<String, FilterOptionCounts>::new();
    let mut preview_games_by_system: HashMap<String, Vec<usize>> = HashMap::new();
    let mut preview_best_by_system = HashMap::<String, HashMap<String, usize>>::new();
    let mut text_indexes = match mode {
        CatalogIndexMode::Eager => build_arcade_text_indexes(games, platform_kinds),
        CatalogIndexMode::DeferredText => ArcadeTextIndexes::default(),
    };

    let total = games.len();
    if let Some(report) = index_progress.as_deref_mut() {
        report(0, total);
    }
    for (idx, game) in games.iter().enumerate() {
        for system_id in game_collection_ids(game, platform_kinds)
            .into_iter()
            .flatten()
        {
            let system_id_string = system_id.to_string();
            let system_id_arc: Arc<str> = Arc::from(system_id);
            games_by_system
                .entry(system_id_string.clone())
                .or_default()
                .push(idx);

            let counts = filter_counts_by_system
                .entry(system_id_string.clone())
                .or_default();
            let category = game.category.trim();
            if !category.is_empty() {
                let category: Arc<str> = Arc::from(category);
                games_by_filter
                    .entry(ArcadeFilterKey {
                        system_id: system_id_arc.clone(),
                        kind: ArcadeFilterKindKey::Category(category.clone()),
                    })
                    .or_default()
                    .push(idx);
                *counts.categories.entry(category.to_string()).or_default() += 1;
            }
            if let Some(year) = game.year {
                let decade = (year / 10) * 10;
                games_by_filter
                    .entry(ArcadeFilterKey {
                        system_id: system_id_arc.clone(),
                        kind: ArcadeFilterKindKey::Decade(decade),
                    })
                    .or_default()
                    .push(idx);
                *counts.decades.entry(decade).or_default() += 1;
            }

            if !game.manufacturer.is_empty() {
                games_by_filter
                    .entry(ArcadeFilterKey {
                        system_id: system_id_arc.clone(),
                        kind: ArcadeFilterKindKey::Manufacturer(game.manufacturer.clone()),
                    })
                    .or_default()
                    .push(idx);
            }
            let manufacturer = game.manufacturer.trim();
            if !manufacturer.is_empty() {
                *counts
                    .manufacturers
                    .entry(manufacturer.to_string())
                    .or_default() += 1;
            }

            if let Some(players) = game.players {
                games_by_filter
                    .entry(ArcadeFilterKey {
                        system_id: system_id_arc.clone(),
                        kind: ArcadeFilterKindKey::Players(players),
                    })
                    .or_default()
                    .push(idx);
                *counts.players.entry(players).or_default() += 1;
            }

            let control = canonical_control_label(&game.control);
            if !control.is_empty() {
                let control: Arc<str> = Arc::from(control);
                games_by_filter
                    .entry(ArcadeFilterKey {
                        system_id: system_id_arc,
                        kind: ArcadeFilterKindKey::Control(control.clone()),
                    })
                    .or_default()
                    .push(idx);
                *counts.controls.entry(control.to_string()).or_default() += 1;
            }

            if has_preview_image(game) {
                let preview_indexes = preview_games_by_system
                    .entry(system_id_string.clone())
                    .or_default();
                let best_by_key = preview_best_by_system.entry(system_id_string).or_default();
                let key = preview_dedupe_key(&game.title);
                if let Some(&preview_pos) = best_by_key.get(&key) {
                    if prefer_preview_game(game, &games[preview_indexes[preview_pos]]) {
                        preview_indexes[preview_pos] = idx;
                    }
                } else {
                    best_by_key.insert(key, preview_indexes.len());
                    preview_indexes.push(idx);
                }
            }
        }
        if let Some(report) = index_progress.as_deref_mut() {
            report(idx + 1, total);
        }
    }

    let filter_options_by_system = filter_counts_by_system
        .into_iter()
        .map(|(system_id, counts)| {
            (
                system_id,
                ArcadeSystemFilterOptions {
                    categories: string_filter_options_from_counts(counts.categories),
                    decades: counts
                        .decades
                        .into_iter()
                        .map(|(decade, count)| ArcadeFilterOption {
                            label: format!("{decade}'s"),
                            count,
                        })
                        .collect(),
                    manufacturers: string_filter_options_from_counts(counts.manufacturers),
                    players: counts
                        .players
                        .into_iter()
                        .map(|(players, count)| ArcadeFilterOption {
                            label: player_count_label(players),
                            count,
                        })
                        .collect(),
                    controls: string_filter_options_from_counts(counts.controls),
                },
            )
        })
        .collect();
    let launch_plans_by_ref = launch_plans
        .into_iter()
        .map(|plan| (plan.launch_ref.clone(), plan))
        .collect();

    ArcadeCatalogIndexes {
        games_by_system,
        games_by_filter,
        filter_options_by_system,
        preview_games_by_system,
        launch_plans_by_ref,
        search_keys: std::mem::take(&mut text_indexes.search_keys),
        autocomplete: std::mem::take(&mut text_indexes.autocomplete),
    }
}

fn build_system_collection_preview_indexes(
    games: &[ArcadeGameEntry],
    platform_kinds: &HashMap<String, PlatformKind>,
) -> HashMap<String, Vec<usize>> {
    let mut preview_games_by_system = HashMap::<String, Vec<usize>>::new();
    let mut preview_best_by_system = HashMap::<String, HashMap<String, usize>>::new();
    for (idx, game) in games.iter().enumerate() {
        if !has_preview_image(game) {
            continue;
        }
        for system_id in game_collection_ids(game, platform_kinds)
            .into_iter()
            .flatten()
        {
            let preview_indexes = preview_games_by_system
                .entry(system_id.to_owned())
                .or_default();
            let best_by_key = preview_best_by_system
                .entry(system_id.to_owned())
                .or_default();
            let key = preview_dedupe_key(&game.title);
            if let Some(&preview_pos) = best_by_key.get(&key) {
                if prefer_preview_game(game, &games[preview_indexes[preview_pos]]) {
                    preview_indexes[preview_pos] = idx;
                }
            } else {
                best_by_key.insert(key, preview_indexes.len());
                preview_indexes.push(idx);
            }
        }
    }
    preview_games_by_system
}

fn build_arcade_text_indexes(
    games: &[ArcadeGameEntry],
    platform_kinds: &HashMap<String, PlatformKind>,
) -> ArcadeTextIndexes {
    build_arcade_text_indexes_with_timing_while(
        games,
        platform_kinds,
        TextIndexBuildPacing::Unthrottled,
        || true,
    )
    .expect("unthrottled text-index build cannot be cancelled")
    .0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextIndexBuildPacing {
    Unthrottled,
    Interactive,
}

impl TextIndexBuildPacing {
    /// Yield only after a real elapsed-time budget. Fixed sleeps made the
    /// indexer take a scheduler-dependent 8+ seconds on large catalogs.
    fn cooperate<F>(
        &self,
        completed_games: usize,
        next_yield: &mut std::time::Instant,
        keep_running: &mut F,
    ) -> bool
    where
        F: FnMut() -> bool,
    {
        // Checking at 64 rows bounds cancellation latency without putting a
        // clock call on every game.
        if !completed_games.is_multiple_of(64) {
            return true;
        }
        if !keep_running() {
            return false;
        }
        if self == &Self::Interactive && std::time::Instant::now() >= *next_yield {
            std::thread::yield_now();
            *next_yield = std::time::Instant::now() + std::time::Duration::from_millis(3);
        }
        true
    }
}

fn build_arcade_text_indexes_with_timing_while<F>(
    games: &[ArcadeGameEntry],
    platform_kinds: &HashMap<String, PlatformKind>,
    pacing: TextIndexBuildPacing,
    mut keep_running: F,
) -> Option<(ArcadeTextIndexes, ArcadeTextIndexBuildTiming)>
where
    F: FnMut() -> bool,
{
    if !keep_running() {
        return None;
    }
    let total_t = std::time::Instant::now();
    let mut search_keys = Vec::with_capacity(games.len());
    let mut autocomplete = ArcadeAutocompleteIndex::default();
    let mut next_yield = total_t + std::time::Duration::from_millis(3);
    // Search keys and autocomplete use the same game fields. Keeping them in
    // one traversal avoids a second 67k-row cache walk and preserves both
    // existing result/ranking structures exactly.
    for (index, game) in games.iter().enumerate() {
        search_keys.push(ArcadeSearchKey::from_game(game));
        autocomplete.add_game(game, platform_kinds);
        if !pacing.cooperate(index + 1, &mut next_yield, &mut keep_running) {
            return None;
        }
    }
    let total_us = total_t.elapsed().as_micros() as u64;
    Some((
        ArcadeTextIndexes {
            search_keys,
            autocomplete,
        },
        ArcadeTextIndexBuildTiming {
            built: false,
            // Fused construction has no meaningful phase boundary. Retain the
            // legacy fields for trace compatibility and attribute all work to
            // the combined search-key phase.
            search_keys_us: total_us,
            autocomplete_us: 0,
            total_us,
        },
    ))
}

impl ArcadeSearchKey {
    fn from_game(game: &ArcadeGameEntry) -> Self {
        let title = search_match_key(&game.title);
        let path = search_match_key(mra_basename(&game.mra_path));
        let manufacturer = search_match_key(&game.manufacturer);
        let category = search_match_key(&game.category);
        let players = game
            .players
            .map(player_count_label)
            .map(|label| search_match_key(&label))
            .unwrap_or_default();
        let control = search_match_key(&canonical_control_label(&game.control));
        let year = game.year.map(|year| year.to_string()).unwrap_or_default();
        let decade = game
            .year
            .map(|year| format!("{}0s", year / 10))
            .unwrap_or_default();
        Self {
            compact_title: title.replace(' ', ""),
            compact_path: path.replace(' ', ""),
            compact_manufacturer: manufacturer.replace(' ', ""),
            compact_category: category.replace(' ', ""),
            compact_control: control.replace(' ', ""),
            title,
            path,
            manufacturer,
            category,
            players,
            control,
            year,
            decade,
        }
    }

    fn score(&self, tokens: &[&str], compact_needle: &str) -> Option<u16> {
        let mut score = 0;
        for token in tokens {
            let token_score = self.score_token(token);
            if token_score == 0 {
                return None;
            }
            score += token_score;
        }
        if !compact_needle.is_empty() {
            score = score.max(search_field_score(&self.compact_title, compact_needle, 92));
            score = score.max(search_field_score(
                &self.compact_manufacturer,
                compact_needle,
                74,
            ));
            score = score.max(search_field_score(
                &self.compact_category,
                compact_needle,
                72,
            ));
            score = score.max(search_field_score(
                &self.compact_control,
                compact_needle,
                64,
            ));
            score = score.max(search_field_score(&self.compact_path, compact_needle, 35));
        }
        (score > 0).then_some(score)
    }

    fn score_token(&self, token: &str) -> u16 {
        search_field_score(&self.title, token, 100)
            .max(search_field_score(&self.manufacturer, token, 80))
            .max(search_field_score(&self.category, token, 76))
            .max(search_field_score(&self.control, token, 70))
            .max(search_field_score(&self.players, token, 68))
            .max(search_field_score(&self.year, token, 65))
            .max(search_field_score(&self.decade, token, 60))
            .max(search_field_score(&self.path, token, 40))
    }
}

fn search_field_score(field: &str, needle: &str, base: u16) -> u16 {
    if needle.is_empty() || field.is_empty() {
        return 0;
    }
    if field == needle {
        base + 20
    } else if field.starts_with(needle) {
        base + 12
    } else if field
        .split_whitespace()
        .any(|word| word == needle || word.starts_with(needle))
    {
        base + 8
    } else if field.contains(needle) {
        base
    } else {
        0
    }
}

#[derive(Clone, Debug, Default)]
struct ArcadeAutocompleteIndex {
    words: HashMap<String, AutocompleteWordStats>,
}

#[derive(Clone, Debug, Default)]
struct AutocompleteWordStats {
    total_score: u32,
    source_rank: u8,
    system_scores: HashMap<String, u32>,
    system_source_ranks: HashMap<String, u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutocompleteSource {
    Title,
    Metadata,
    Path,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AutocompleteCandidate {
    word: String,
    current_system: bool,
    source_rank: u8,
    system_score: u32,
    total_score: u32,
}

impl Ord for AutocompleteCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.current_system
            .cmp(&other.current_system)
            .then_with(|| self.source_rank.cmp(&other.source_rank))
            .then_with(|| self.system_score.cmp(&other.system_score))
            .then_with(|| self.total_score.cmp(&other.total_score))
            .then_with(|| other.word.cmp(&self.word))
    }
}

impl PartialOrd for AutocompleteCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl ArcadeAutocompleteIndex {
    fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    fn add_game(&mut self, game: &ArcadeGameEntry, platform_kinds: &HashMap<String, PlatformKind>) {
        for system_id in game_collection_ids(game, platform_kinds)
            .into_iter()
            .flatten()
        {
            self.add_game_for_system(system_id, game);
        }
    }

    fn add_game_for_system(&mut self, system_id: &str, game: &ArcadeGameEntry) {
        self.add_words(system_id, &game.title, AutocompleteSource::Title);
        self.add_words(system_id, &game.manufacturer, AutocompleteSource::Metadata);
        self.add_words(system_id, &game.category, AutocompleteSource::Metadata);
        self.add_words(
            system_id,
            &canonical_control_label(&game.control),
            AutocompleteSource::Metadata,
        );
        if let Some(players) = game.players {
            self.add_word(
                system_id,
                &player_count_label(players),
                AutocompleteSource::Metadata,
            );
        }
        self.add_words(
            system_id,
            mra_basename(&game.mra_path),
            AutocompleteSource::Path,
        );
        if let Some(year) = game.year {
            self.add_word(system_id, &year.to_string(), AutocompleteSource::Metadata);
            self.add_word(
                system_id,
                &format!("{}0s", year / 10),
                AutocompleteSource::Metadata,
            );
        }
    }

    fn add_words(&mut self, system_id: &str, value: &str, source: AutocompleteSource) {
        for word in autocomplete_words(value) {
            self.add_word(system_id, &word, source);
        }
    }

    fn add_word(&mut self, system_id: &str, word: &str, source: AutocompleteSource) {
        if word.len() < 2 || is_noisy_autocomplete_word(word) {
            return;
        }
        let score = match source {
            AutocompleteSource::Title => 5,
            AutocompleteSource::Metadata => 4,
            AutocompleteSource::Path => 1,
        };
        let source_rank = match source {
            AutocompleteSource::Title | AutocompleteSource::Metadata => 2,
            AutocompleteSource::Path => 1,
        };
        let stats = self.words.entry(word.to_string()).or_default();
        stats.total_score += score;
        stats.source_rank = stats.source_rank.max(source_rank);
        *stats
            .system_scores
            .entry(system_id.to_string())
            .or_default() += score;
        let system_source_rank = stats
            .system_source_ranks
            .entry(system_id.to_string())
            .or_default();
        *system_source_rank = (*system_source_rank).max(source_rank);
    }

    fn suggest(&self, system_id: &str, fragment: &str) -> Option<String> {
        let prefix = search_match_key(fragment);
        if prefix.is_empty() {
            return None;
        }
        self.words
            .iter()
            .filter(|(word, _)| word.starts_with(&prefix))
            .map(|(word, stats)| AutocompleteCandidate {
                word: word.clone(),
                current_system: stats.system_scores.contains_key(system_id),
                source_rank: stats
                    .system_source_ranks
                    .get(system_id)
                    .copied()
                    .unwrap_or(stats.source_rank),
                system_score: stats
                    .system_scores
                    .get(system_id)
                    .copied()
                    .unwrap_or_default(),
                total_score: stats.total_score,
            })
            .max()
            .map(|candidate| candidate.word)
    }
}

fn filter_key(system_id: &str, filter: &ArcadeFilter) -> Option<ArcadeFilterKey> {
    match filter {
        ArcadeFilter::All | ArcadeFilter::Search => None,
        ArcadeFilter::Category(category) => Some(ArcadeFilterKey {
            system_id: Arc::from(system_id),
            kind: ArcadeFilterKindKey::Category(Arc::from(category.trim())),
        }),
        ArcadeFilter::Decade(decade) => Some(ArcadeFilterKey {
            system_id: Arc::from(system_id),
            kind: ArcadeFilterKindKey::Decade(*decade),
        }),
        ArcadeFilter::Manufacturer(manufacturer) => Some(ArcadeFilterKey {
            system_id: Arc::from(system_id),
            kind: ArcadeFilterKindKey::Manufacturer(Arc::from(manufacturer.as_str())),
        }),
        ArcadeFilter::Players(players) => Some(ArcadeFilterKey {
            system_id: Arc::from(system_id),
            kind: ArcadeFilterKindKey::Players(*players),
        }),
        ArcadeFilter::Control(control) => Some(ArcadeFilterKey {
            system_id: Arc::from(system_id),
            kind: ArcadeFilterKindKey::Control(Arc::from(canonical_control_label(control))),
        }),
    }
}

pub(crate) fn player_count_label(players: u8) -> String {
    if players == 0 {
        "Unkown".to_string()
    } else if players == 1 {
        "1 Player".to_string()
    } else {
        format!("{players} Players")
    }
}

pub(crate) fn canonical_control_label(control: &str) -> String {
    let control = control.trim();
    match control.to_ascii_lowercase().as_str() {
        "" => String::new(),
        "joy" | "joystick" => "Joystick".to_string(),
        "doublejoy" | "dual joystick" => "Dual Joystick".to_string(),
        "triplejoy" | "triple joystick" => "Triple Joystick".to_string(),
        "only_buttons" | "buttons only" => "Buttons Only".to_string(),
        "paddle" => "Paddle".to_string(),
        "dial" => "Dial".to_string(),
        "trackball" => "Trackball".to_string(),
        "lightgun" | "light gun" => "Light Gun".to_string(),
        "positional" => "Positional".to_string(),
        "stick" => "Stick".to_string(),
        "keyboard" => "Keyboard".to_string(),
        "keypad" => "Keypad".to_string(),
        "mouse" => "Mouse".to_string(),
        "pedal" => "Pedal".to_string(),
        "mahjong" | "mahjong panel" => "Mahjong Panel".to_string(),
        "hanafuda" | "hanafuda panel" => "Hanafuda Panel".to_string(),
        "gambling" | "gambling controls" => "Gambling Controls".to_string(),
        other => other
            .split(['_', '-'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .map(|first| format!("{}{}", first.to_ascii_uppercase(), chars.as_str()))
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn mra_basename(path: &str) -> &str {
    path.rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(".mra").or(Some(name)))
        .unwrap_or(path)
}

fn search_match_key(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compact_search_match_key(value: &str) -> String {
    search_match_key(value).replace(' ', "")
}

fn search_query_tokens(query: &str) -> Vec<&str> {
    query.split_whitespace().collect()
}

fn current_search_word(query: &str) -> &str {
    query
        .rsplit_once(char::is_whitespace)
        .map(|(_, word)| word)
        .unwrap_or(query)
}

fn autocomplete_words(value: &str) -> Vec<String> {
    search_match_key(value)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn is_noisy_autocomplete_word(word: &str) -> bool {
    matches!(
        word,
        "a" | "an"
            | "and"
            | "the"
            | "of"
            | "in"
            | "on"
            | "to"
            | "for"
            | "with"
            | "world"
            | "usa"
            | "us"
            | "europe"
            | "japan"
            | "rev"
            | "revision"
            | "version"
            | "ver"
            | "set"
            | "en"
            | "fr"
            | "es"
            | "it"
            | "de"
            | "ocs"
            | "aga"
    )
}

#[derive(Default)]
struct FilterOptionCounts {
    categories: BTreeMap<String, usize>,
    decades: BTreeMap<u16, usize>,
    manufacturers: BTreeMap<String, usize>,
    players: BTreeMap<u8, usize>,
    controls: BTreeMap<String, usize>,
}

fn string_filter_options_from_counts(counts: BTreeMap<String, usize>) -> Vec<ArcadeFilterOption> {
    counts
        .into_iter()
        .map(|(label, count)| ArcadeFilterOption { label, count })
        .collect()
}

#[cfg(test)]
fn preview_games(games: &[ArcadeGameEntry], indexes: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut best_idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<usize> = Vec::new();

    for index in indexes {
        let Some(game) = games.get(index) else {
            continue;
        };
        if !has_preview_image(game) {
            continue;
        }
        let key = preview_dedupe_key(&game.title);
        if let Some(&idx) = best_idx.get(&key) {
            if prefer_preview_game(game, &games[out[idx]]) {
                out[idx] = index;
            }
        } else {
            best_idx.insert(key, out.len());
            out.push(index);
        }
    }

    out
}

fn preview_dedupe_key(title: &str) -> String {
    let base = title
        .split_once('(')
        .map(|(before, _)| before.trim())
        .filter(|before| !before.is_empty())
        .unwrap_or(title);
    base.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn prefer_preview_game(a: &ArcadeGameEntry, b: &ArcadeGameEntry) -> bool {
    let a_exact = !a.title.contains('(');
    let b_exact = !b.title.contains('(');
    if a_exact != b_exact {
        return a_exact;
    }
    if a.title.len() != b.title.len() {
        return a.title.len() < b.title.len();
    }
    a.mra_path < b.mra_path
}

fn has_preview_image(game: &ArcadeGameEntry) -> bool {
    game.has_preview && !game.preview_archive_path.is_empty() && !game.preview_asset_key.is_empty()
}

pub fn systems_from_games(games: &[ArcadeGameEntry]) -> Vec<GameSystemEntry> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for game in games {
        *counts.entry(game.system_id.to_string()).or_default() += 1;
    }
    let mut systems: Vec<GameSystemEntry> = counts
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(id, count)| GameSystemEntry {
            title: system_title(&id),
            id,
            count,
        })
        .collect();
    sort_systems_by_title(&mut systems);
    systems
}

fn inferred_platform_kinds(systems: &[GameSystemEntry]) -> HashMap<String, PlatformKind> {
    systems
        .iter()
        .map(|system| {
            (
                system.id.clone(),
                PlatformKind::inferred_for_system_id(&system.id),
            )
        })
        .collect()
}

fn fill_missing_platform_kinds(
    systems: &[GameSystemEntry],
    platform_kinds: &mut HashMap<String, PlatformKind>,
) {
    for system in systems {
        platform_kinds
            .entry(system.id.clone())
            .or_insert_with(|| PlatformKind::inferred_for_system_id(&system.id));
    }
}

fn sort_systems_by_title(systems: &mut [GameSystemEntry]) {
    systems.sort_by_cached_key(|system| (system.title.to_ascii_lowercase(), system.id.clone()));
}

pub fn system_title(id: &str) -> String {
    if let Some(definition) = crate::catalog_classify::system_definition(id) {
        return definition.title.clone();
    }
    match id {
        "acornatom" => "Acorn Atom".to_string(),
        "acornelectron" => "Acorn Electron".to_string(),
        "altair8800" => "Altair 8800".to_string(),
        "amiga" => "Amiga".to_string(),
        "amstrad" => "Amstrad".to_string(),
        "apple-ii" => "Apple II".to_string(),
        "aquarius" => "Aquarius".to_string(),
        "arcade" => "Arcade".to_string(),
        "arcadia" => "Arcadia".to_string(),
        "archie" => "Archie".to_string(),
        "atari2600" => "Atari 2600".to_string(),
        "atari5200" => "Atari 5200".to_string(),
        "atari7800" => "Atari 7800".to_string(),
        "atari800" => "Atari 800".to_string(),
        "amigacd32" => "Amiga CD32".to_string(),
        "atarilynx" => "Atari Lynx".to_string(),
        "atarist" => "Atari ST".to_string(),
        "bbcmicro" => "BBC Micro".to_string(),
        "c128" => "C128".to_string(),
        "c16" => "C16".to_string(),
        "c64" => "C64".to_string(),
        "casio_pv-1000" => "Casio PV-1000".to_string(),
        "casio_pv-2000" => "Casio PV-2000".to_string(),
        "channelf" => "Channel F".to_string(),
        "coco2" => "CoCo 2".to_string(),
        "coco3" => "CoCo 3".to_string(),
        "colecovision" => "ColecoVision".to_string(),
        "creativision" => "CreatiVision".to_string(),
        "neogeo" | "neo-geo" | "snk-neo-geo" => "NeoGeo".to_string(),
        "neogeo-cd" => "NeoGeo CD".to_string(),
        "neogeopocket" => "NeoGeo Pocket".to_string(),
        "cps1" | "capcom-cps1" => "CPS1".to_string(),
        "cps2" | "capcom-cps2" => "CPS2".to_string(),
        "cps3" | "capcom-cps3" => "CPS3".to_string(),
        "system16" | "sega-system16" => "System 16".to_string(),
        "system18" | "sega-system18" => "System 18".to_string(),
        "m72" | "irem-m72" => "Irem M72".to_string(),
        "m92" | "irem-m92" => "Irem M92".to_string(),
        "gameboy" => "Game Boy".to_string(),
        "eg2000" => "EG2000".to_string(),
        "gba" => "GBA".to_string(),
        "gba2p" => "GBA 2P".to_string(),
        "gbc" => "Game Boy Color".to_string(),
        "gb" => "Game Boy".to_string(),
        "nes" => "NES".to_string(),
        "snes" => "SNES".to_string(),
        "n64" => "Nintendo 64".to_string(),
        "sms" => "Sega Master System".to_string(),
        "psx" => "PlayStation".to_string(),
        "ao486" => "AO486".to_string(),
        "dos" => "DOS Games".to_string(),
        "megadrive" => "Mega Drive".to_string(),
        "megacd" => "Mega CD".to_string(),
        "s32x" => "Sega 32X".to_string(),
        "gamegear" => "Game Gear".to_string(),
        "intellivision" => "Intellivision".to_string(),
        "jaguar" => "Jaguar".to_string(),
        "maclc" => "Mac LC".to_string(),
        "macplus" => "Mac Plus".to_string(),
        "msx" => "MSX".to_string(),
        "odyssey2" => "Odyssey 2".to_string(),
        "oric" => "Oric".to_string(),
        "pc88" => "PC-8801".to_string(),
        "pet2001" => "PET 2001".to_string(),
        "pokemonmini" => "Pokemon Mini".to_string(),
        "ql" => "QL".to_string(),
        "samcoupe" => "SAM Coupe".to_string(),
        "saturn" => "Saturn".to_string(),
        "sgb" => "Super Game Boy".to_string(),
        "supervision" => "Supervision".to_string(),
        "tomytutor" => "Tomy Tutor".to_string(),
        "trs-80" => "TRS-80".to_string(),
        "vectrex" => "Vectrex".to_string(),
        "vic20" => "VIC-20".to_string(),
        "wonderswan" => "WonderSwan".to_string(),
        "x68000" => "X68000".to_string(),
        "zx-spectrum" => "ZX Spectrum".to_string(),
        "zx81" => "ZX81".to_string(),
        "unknown" => "Unknown".to_string(),
        other => other
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "builder")]
    fn navpack_fixture(rows: usize) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-mapped-system-{}-{}.navpack",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let games = (0..rows)
            .map(|ordinal| {
                let title = format!("Game {ordinal:03}");
                let launch_ref = format!("magik-plan:c64:{ordinal}");
                crate::system_shard::SystemGame {
                    stable_key: format!("c64:{ordinal}"),
                    title: title.clone(),
                    launch_ref: launch_ref.clone(),
                    preview_archive_path: "/preview/c64.zip".into(),
                    preview_asset_key: title.clone(),
                    has_preview: ordinal % 3 == 0,
                    year: Some((1980 + ordinal % 10) as u16),
                    manufacturer: "Fixture".into(),
                    category: "Action".into(),
                    players: Some(1),
                    control: "Joystick".into(),
                    is_new: false,
                    launch_plan: Some(crate::system_shard::SystemLaunchPlan {
                        launch_ref,
                        title,
                        system_id: "c64".into(),
                        core_path: "C64".into(),
                        payload_path: format!("/games/{ordinal}.d64"),
                        mount_kind: "mount-image".into(),
                        mount_index: 0,
                        delay_secs: 1,
                    }),
                }
            })
            .collect::<Vec<_>>();
        let indexes = crate::system_shard::build_navigation_indexes(&games).unwrap();
        let encoded = crate::navpack::encode("c64", 7, &games, &indexes).unwrap();
        std::fs::write(&path, encoded).unwrap();
        path
    }

    #[cfg(feature = "builder")]
    #[test]
    fn navpack_collection_materializes_only_the_first_viewport_and_preserves_row_semantics() {
        let path = navpack_fixture(130);
        let bytes = std::fs::metadata(&path).unwrap().len();
        let (collection, timing) =
            SystemCollection::open_navpack("c64", &path, bytes, 7, 130, PlatformKind::Computer)
                .unwrap();
        let SystemCollectionRows::NavPack(rows) = &collection.games else {
            panic!("expected NavPack collection");
        };
        assert_eq!(rows.resident_rows(), NavPackSystemRows::FIRST_VIEWPORT_ROWS);
        assert!(timing.first_viewport_us <= timing.total_us);
        assert_eq!(collection.game_at(129).unwrap().title.as_ref(), "Game 129");
        assert_eq!(collection.metadata_at(129).unwrap().year, Some(1989));
        assert_eq!(
            collection
                .launch_plan_for_ref("magik-plan:c64:129")
                .unwrap()
                .payload_path
                .as_ref(),
            "/games/129.d64"
        );
        assert_eq!(collection.games.iter().count(), 130);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn replacing_one_system_keeps_other_rows_and_launch_plans() {
        let old_plan = StructuredLaunchPlan {
            launch_ref: "magik-plan:c64".into(),
            title: "C64".into(),
            system_id: "c64".into(),
            core_path: "C64".into(),
            payload_path: "/games/C64/old.d64".into(),
            mount_kind: "mount-image".into(),
            mount_index: 0,
            delay_secs: 1,
        };
        let retained_plan = StructuredLaunchPlan {
            launch_ref: "magik-plan:snes".into(),
            title: "SNES".into(),
            system_id: "snes".into(),
            core_path: "SNES".into(),
            payload_path: "/games/SNES/keep.sfc".into(),
            mount_kind: "load-file".into(),
            mount_index: 0,
            delay_secs: 1,
        };
        let game = |title: &str, launch_ref: &str, system_id: &str| ArcadeGameEntry {
            title: title.into(),
            mra_path: launch_ref.into(),
            preview_archive_path: "".into(),
            preview_asset_key: "".into(),
            has_preview: false,
            system_id: system_id.into(),
            year: None,
            manufacturer: "".into(),
            category: "".into(),
            players: None,
            control: "".into(),
            is_new: false,
        };
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from("/fixture"),
            vec![
                game("Old", "magik-plan:c64", "c64"),
                game("Keep", "magik-plan:snes", "snes"),
            ],
            vec![
                GameSystemEntry {
                    id: "c64".into(),
                    title: "C64".into(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "snes".into(),
                    title: "SNES".into(),
                    count: 1,
                },
            ],
            vec![old_plan, retained_plan.clone()],
        );
        let replacement_plan = StructuredLaunchPlan {
            launch_ref: "magik-plan:c64-new".into(),
            payload_path: "/games/C64/new.d64".into(),
            ..retained_plan.clone()
        };

        let replaced = catalog.replacing_system_games(
            "c64",
            vec![game("New", "magik-plan:c64-new", "c64")],
            vec![replacement_plan],
        );

        assert_eq!(replaced.system_game_count("c64"), 1);
        assert_eq!(replaced.system_game_count("snes"), 1);
        assert!(matches!(
            replaced.launch_target_for_ref("magik-plan:c64-new"),
            LaunchTarget::Structured(_)
        ));
        assert!(matches!(
            replaced.launch_target_for_ref("magik-plan:snes"),
            LaunchTarget::Structured(_)
        ));
        assert!(Arc::ptr_eq(&catalog.games, &replaced.games));
        assert_eq!(catalog.games[0].title.as_ref(), "Old");
        assert_eq!(
            replaced.system_game_at("c64", 0).unwrap().title.as_ref(),
            "New"
        );
    }

    #[test]
    fn collection_alias_keeps_arcade_artifact_row_identity() {
        let base = ArcadeCatalog::new(
            PathBuf::from(DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![GameSystemEntry {
                id: "arcade".into(),
                title: "Arcade".into(),
                count: 1,
            }],
        );
        let collection = Arc::new(SystemCollection::new(
            "arcade",
            vec![ArcadeGameEntry {
                title: "Fixture".into(),
                mra_path: "/media/fat/_Arcade/Fixture.mra".into(),
                preview_archive_path: "/media/fat/preview/arcade.zip".into(),
                preview_asset_key: "Fixture".into(),
                has_preview: true,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                players: None,
                control: "".into(),
                is_new: false,
            }],
            Vec::new(),
            PlatformKind::Arcade,
        ));

        let catalog = base.with_system_collection_for_id(MENU_ARCADE_SYSTEM_ID, collection);

        assert_eq!(catalog.system_game_count(MENU_ARCADE_SYSTEM_ID), 1);
        assert_eq!(
            catalog
                .system_game_at(MENU_ARCADE_SYSTEM_ID, 0)
                .unwrap()
                .system_id
                .as_ref(),
            "arcade"
        );
        assert_eq!(catalog.system_game_count("arcade"), 0);
    }

    #[test]
    fn system_collection_routes_rows_filters_previews_and_launches() {
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from("/fixture"),
            Vec::new(),
            vec![GameSystemEntry {
                id: "c64".into(),
                title: "Commodore 64".into(),
                count: 0,
            }],
            Vec::new(),
        );
        let game = ArcadeGameEntry {
            title: "New C64 Game".into(),
            mra_path: "magik-plan:c64:new".into(),
            preview_archive_path: "/preview/c64.zip".into(),
            preview_asset_key: "New C64 Game".into(),
            has_preview: true,
            system_id: "c64".into(),
            year: Some(1987),
            manufacturer: "Example".into(),
            category: "Action".into(),
            players: Some(1),
            control: "joy".into(),
            is_new: true,
        };
        let plan = StructuredLaunchPlan {
            launch_ref: game.mra_path.clone(),
            title: game.title.clone(),
            system_id: game.system_id.clone(),
            core_path: "C64".into(),
            payload_path: "/games/C64/New C64 Game.d64".into(),
            mount_kind: "mount-image".into(),
            mount_index: 0,
            delay_secs: 1,
        };

        let collection = Arc::new(SystemCollection::new(
            "c64",
            vec![game],
            vec![plan],
            PlatformKind::Computer,
        ));

        assert!(collection.rich_indexes.get().is_none());
        assert!(collection.games.get(0).unwrap().manufacturer.is_empty());
        assert!(matches!(
            collection
                .launch_plans
                .binary_search_by(|plan| plan.launch_ref.as_ref().cmp("magik-plan:c64:new")),
            Ok(_)
        ));
        let hydrated = catalog.with_system_collection(collection.clone());

        assert_eq!(hydrated.system_game_count("c64"), 1);
        assert_eq!(
            hydrated.system_game_at("c64", 0).unwrap().title.as_ref(),
            "New C64 Game"
        );
        assert_eq!(
            hydrated.system_game_metadata_at("c64", 0),
            Some(ArcadeGameMetadataKey {
                year: Some(1987),
                manufacturer: "Example".into(),
                category: "Action".into(),
                players: Some(1),
                control: "joy".into(),
            })
        );
        assert!(collection.rich_indexes.get().is_none());
        assert_eq!(
            hydrated.filtered_game_count("c64", &ArcadeFilter::Category("Action".into())),
            1
        );
        assert_eq!(hydrated.category_options("c64")[0].label, "Action");
        assert!(collection.rich_indexes.get().is_some());
        assert_eq!(hydrated.system_preview_game_count("c64"), 1);
        assert_eq!(
            hydrated
                .system_preview_game_at("c64", 0)
                .unwrap()
                .preview_asset_key
                .as_ref(),
            "New C64 Game"
        );
        assert!(matches!(
            hydrated.launch_target_for_ref("magik-plan:c64:new"),
            LaunchTarget::Structured(_)
        ));
        assert_eq!(hydrated.search_source_system_ids("c64"), vec!["c64"]);
    }

    #[test]
    fn empty_placeholder_cleanup_preserves_registered_and_resident_systems() {
        let resident_game = ArcadeGameEntry {
            title: "1942".into(),
            mra_path: "/media/fat/_Arcade/1942.mra".into(),
            preview_archive_path: "".into(),
            preview_asset_key: "".into(),
            has_preview: false,
            system_id: "arcade".into(),
            year: None,
            manufacturer: "".into(),
            category: "".into(),
            players: None,
            control: "".into(),
            is_new: false,
        };
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds(
            PathBuf::from("/fixture"),
            vec![resident_game],
            vec![
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 0,
                },
                GameSystemEntry {
                    id: "snes".into(),
                    title: "SNES".into(),
                    count: 4,
                },
                GameSystemEntry {
                    id: "apple-iigs".into(),
                    title: "APPLE-IIGS".into(),
                    count: 0,
                },
            ],
            Vec::new(),
            HashMap::from([
                ("arcade".to_string(), PlatformKind::Arcade),
                ("snes".to_string(), PlatformKind::Console),
                ("apple-iigs".to_string(), PlatformKind::Unknown),
            ]),
        )
        .with_projection_stats(HashMap::from([
            (
                "arcade".to_string(),
                SystemProjectionStats {
                    source_games: 1,
                    visible_families: 1,
                    collapsed_variants: 0,
                },
            ),
            (
                "apple-iigs".to_string(),
                SystemProjectionStats {
                    source_games: 0,
                    visible_families: 0,
                    collapsed_variants: 0,
                },
            ),
        ]));

        let cleaned = catalog.without_empty_system_placeholders();

        assert_eq!(
            cleaned
                .systems
                .iter()
                .map(|system| system.id.as_str())
                .collect::<Vec<_>>(),
            vec!["arcade", "snes"]
        );
        assert_eq!(cleaned.system_game_count("arcade"), 1);
        assert_eq!(cleaned.system_game_count("snes"), 0);
        assert_eq!(cleaned.platform_kind("arcade"), PlatformKind::Arcade);
        assert_eq!(cleaned.platform_kind("snes"), PlatformKind::Console);
        assert!(!cleaned.platform_kinds.contains_key("apple-iigs"));
        assert!(cleaned.projection_stats_by_system.contains_key("arcade"));
        assert!(
            !cleaned
                .projection_stats_by_system
                .contains_key("apple-iigs")
        );
    }

    #[test]
    fn adding_a_placeholder_shares_existing_catalog_indexes() {
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from("/fixture"),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );

        let with_placeholder = catalog.with_system_placeholder("snes");

        assert!(Arc::ptr_eq(&catalog.games, &with_placeholder.games));
        assert!(Arc::ptr_eq(
            &catalog.games_by_system,
            &with_placeholder.games_by_system
        ));
        assert!(Arc::ptr_eq(
            &catalog.games_by_filter,
            &with_placeholder.games_by_filter
        ));
        assert_eq!(with_placeholder.systems.len(), 1);
        assert_eq!(with_placeholder.systems[0].id, "snes");
    }

    #[cfg(feature = "builder")]
    #[test]
    fn isolated_system_catalog_keeps_exact_rows_launch_plans_and_platform_kind() {
        let arcade_plan = StructuredLaunchPlan {
            launch_ref: "magik-plan:arcade".into(),
            title: "1942".into(),
            system_id: "arcade".into(),
            core_path: "Arcade/1942".into(),
            payload_path: "/media/fat/_Arcade/1942.mra".into(),
            mount_kind: "launch-core".into(),
            mount_index: 0,
            delay_secs: 0,
        };
        let console_plan = StructuredLaunchPlan {
            launch_ref: "magik-plan:snes".into(),
            title: "F-Zero".into(),
            system_id: "snes".into(),
            core_path: "SNES".into(),
            payload_path: "/media/fat/games/SNES/F-Zero.sfc".into(),
            mount_kind: "load-file".into(),
            mount_index: 0,
            delay_secs: 1,
        };
        let game = |title: &str, launch_ref: &str, system_id: &str| ArcadeGameEntry {
            title: title.into(),
            mra_path: launch_ref.into(),
            preview_archive_path: "".into(),
            preview_asset_key: "".into(),
            has_preview: false,
            system_id: system_id.into(),
            year: None,
            manufacturer: "".into(),
            category: "".into(),
            players: None,
            control: "".into(),
            is_new: false,
        };
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds(
            PathBuf::from("/fixture"),
            vec![
                game("1942", &arcade_plan.launch_ref, "arcade"),
                game("F-Zero", &console_plan.launch_ref, "snes"),
            ],
            vec![
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "snes".into(),
                    title: "SNES".into(),
                    count: 1,
                },
            ],
            vec![arcade_plan.clone(), console_plan],
            HashMap::from([
                ("arcade".to_string(), PlatformKind::Arcade),
                ("snes".to_string(), PlatformKind::Console),
            ]),
        );

        let isolated = catalog.isolated_system_catalog("arcade");

        assert_eq!(isolated.games.len(), 1);
        assert_eq!(isolated.games[0].title.as_ref(), "1942");
        assert_eq!(
            isolated.games[0].mra_path.as_ref(),
            arcade_plan.launch_ref.as_ref()
        );
        assert_eq!(isolated.games[0].system_id.as_ref(), "arcade");
        assert_eq!(isolated.systems.len(), 1);
        assert_eq!(isolated.systems[0].id, "arcade");
        assert_eq!(isolated.platform_kind("arcade"), PlatformKind::Arcade);
        assert_eq!(
            isolated.structured_launch_plan_for_ref(&arcade_plan.launch_ref),
            Some(&arcade_plan)
        );
        assert!(
            isolated
                .structured_launch_plan_for_ref("magik-plan:snes")
                .is_none()
        );
    }
    use crate::test_support::arcade_game;

    fn game(
        title: &str,
        mra_path: &str,
        preview_asset_key: &str,
        system_id: &str,
    ) -> ArcadeGameEntry {
        let game = arcade_game(title).path(mra_path).system_id(system_id);
        if preview_asset_key.is_empty() {
            game.build()
        } else {
            game.preview(preview_asset_key).build()
        }
    }

    #[test]
    fn preview_games_require_images_and_collapse_parenthetical_clones() {
        let root = PathBuf::from("/media/fat/_Arcade");
        let systems = vec![GameSystemEntry {
            id: "arcade".into(),
            title: "Arcade".into(),
            count: 5,
        }];
        let games = vec![
            ArcadeGameEntry {
                title: "1941: Counter Attack (Japan)".into(),
                mra_path: "/media/fat/_Arcade/1941 Japan.mra".into(),
                preview_archive_path: "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b"
                    .into(),
                preview_asset_key: "1941u".into(),
                has_preview: true,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                players: None,
                control: "".into(),
                is_new: false,
            },
            ArcadeGameEntry {
                title: "1941: Counter Attack (World)".into(),
                mra_path: "/media/fat/_Arcade/1941 World.mra".into(),
                preview_archive_path: "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b"
                    .into(),
                preview_asset_key: "1941u".into(),
                has_preview: true,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                players: None,
                control: "".into(),
                is_new: false,
            },
            ArcadeGameEntry {
                title: "1942".into(),
                mra_path: "/media/fat/_Arcade/1942.mra".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                players: None,
                control: "".into(),
                is_new: false,
            },
            ArcadeGameEntry {
                title: "1943".into(),
                mra_path: "/media/fat/_Arcade/1943.mra".into(),
                preview_archive_path: "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b"
                    .into(),
                preview_asset_key: "1943".into(),
                has_preview: true,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                players: None,
                control: "".into(),
                is_new: false,
            },
            ArcadeGameEntry {
                title: "Astra SuperStars".into(),
                mra_path: "/media/fat/_Arcade/Astra SuperStars.mra".into(),
                preview_archive_path: "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b"
                    .into(),
                preview_asset_key: "astrass".into(),
                has_preview: true,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                players: None,
                control: "".into(),
                is_new: false,
            },
        ];
        let catalog = ArcadeCatalog::new(root, games, systems);

        let games = catalog.system_preview_games("arcade");
        assert_eq!(games.len(), 3);
        assert_eq!(catalog.system_preview_game_count("arcade"), 3);
        assert_eq!(games[0].title.as_ref(), "1941: Counter Attack (Japan)");
        assert_eq!(games[1].title.as_ref(), "1943");
        assert_eq!(games[2].title.as_ref(), "Astra SuperStars");
        assert_eq!(
            catalog
                .system_preview_game_at("arcade", 1)
                .map(|game| game.title.to_string()),
            Some("1943".to_string())
        );
    }

    #[test]
    fn system_game_count_includes_games_without_preview_assets() {
        let root = PathBuf::from("/media/fat/_Arcade");
        let systems = vec![GameSystemEntry {
            id: "amiga".into(),
            title: "Amiga".into(),
            count: 1,
        }];
        let games = vec![ArcadeGameEntry {
            title: "Agony".into(),
            mra_path: "magik-plan:amiga-agony".into(),
            preview_archive_path: "".into(),
            preview_asset_key: "".into(),
            has_preview: false,
            system_id: "amiga".into(),
            year: None,
            manufacturer: "".into(),
            category: "".into(),
            players: None,
            control: "".into(),
            is_new: false,
        }];
        let catalog = ArcadeCatalog::new(root, games, systems);

        assert_eq!(catalog.system_game_count("amiga"), 1);
        assert_eq!(catalog.system_game_view("amiga").len(), 1);
        assert_eq!(catalog.system_preview_game_count("amiga"), 0);
    }

    #[test]
    fn amigavision_refs_are_classified_as_prepared_targets() {
        let catalog = ArcadeCatalog::new(PathBuf::new(), Vec::new(), Vec::new());

        assert!(matches!(
            catalog.launch_target_for_ref("magik-amigavision:Agony"),
            LaunchTarget::Prepared(PreparedLaunchSelection {
                collection_id: PreparedCollectionId::AmigaVision,
                ..
            })
        ));
        assert!(matches!(
            catalog.launch_target_for_ref("magik-amigavision-launcher"),
            LaunchTarget::Prepared(_)
        ));
    }

    #[test]
    fn systems_are_strictly_sorted_by_display_title() {
        let deployed_system_ids = [
            "amiga",
            "arcade",
            "atari5200",
            "atari7800",
            "atarilynx",
            "colecovision",
            "dos",
            "gameboy",
            "gba",
            "intellivision",
            "megadrive",
            "n64",
            "neogeo",
            "neogeo-cd",
            "neogeopocket",
            "nes",
            "psx",
            "s32x",
            "saturn",
            "sms",
            "snes",
            "vectrex",
            "wonderswan",
        ];
        let games = deployed_system_ids
            .iter()
            .map(|system_id| game(system_id, &format!("/games/{system_id}.mgl"), "", system_id))
            .collect::<Vec<_>>();

        let systems = systems_from_games(&games)
            .into_iter()
            .map(|system| (system.id, system.title))
            .collect::<Vec<_>>();

        assert_eq!(
            systems,
            vec![
                ("amiga".to_string(), "Amiga".to_string()),
                ("arcade".to_string(), "Arcade".to_string()),
                ("atari5200".to_string(), "Atari 5200".to_string()),
                ("atari7800".to_string(), "Atari 7800".to_string()),
                ("atarilynx".to_string(), "Atari Lynx".to_string()),
                ("colecovision".to_string(), "ColecoVision".to_string()),
                ("dos".to_string(), "DOS Games".to_string()),
                ("gameboy".to_string(), "Game Boy".to_string()),
                ("gba".to_string(), "GBA".to_string()),
                ("intellivision".to_string(), "Intellivision".to_string()),
                ("megadrive".to_string(), "Mega Drive".to_string()),
                ("neogeo".to_string(), "NeoGeo".to_string()),
                ("neogeo-cd".to_string(), "NeoGeo CD".to_string()),
                ("neogeopocket".to_string(), "NeoGeo Pocket".to_string()),
                ("nes".to_string(), "NES".to_string()),
                ("n64".to_string(), "Nintendo 64".to_string()),
                ("psx".to_string(), "PlayStation".to_string()),
                ("saturn".to_string(), "Saturn".to_string()),
                ("s32x".to_string(), "Sega 32X".to_string()),
                ("sms".to_string(), "Sega Master System".to_string()),
                ("snes".to_string(), "SNES".to_string()),
                ("vectrex".to_string(), "Vectrex".to_string()),
                ("wonderswan".to_string(), "WonderSwan".to_string()),
            ]
        );
    }

    #[test]
    fn deployed_runtime_system_ids_have_human_display_titles() {
        let expected = [
            ("acornatom", "Acorn Atom"),
            ("acornelectron", "Acorn Electron"),
            ("bbcmicro", "BBC Micro"),
            ("casio_pv-2000", "Casio PV-2000"),
            ("channelf", "Channel F"),
            ("maclc", "Mac LC"),
            ("pc88", "PC-8801"),
            ("pet2001", "PET 2001"),
            ("amigacd32", "Amiga CD32"),
            ("sgb", "Super Game Boy"),
            ("tomytutor", "Tomy Tutor"),
            ("vic20", "VIC-20"),
            ("zx-spectrum", "ZX Spectrum"),
        ];
        for (id, title) in expected {
            assert_eq!(system_title(id), title, "{id}");
        }
    }

    #[test]
    fn catalog_lookup_falls_back_cleanly_for_missing_paths_and_systems() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            vec![ArcadeGameEntry {
                title: "1942".into(),
                mra_path: "/media/fat/_Arcade/1942.mra".into(),
                preview_archive_path: "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b"
                    .into(),
                preview_asset_key: "1942".into(),
                has_preview: true,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                players: None,
                control: "".into(),
                is_new: false,
            }],
            vec![GameSystemEntry {
                id: "arcade".into(),
                title: "Arcade".into(),
                count: 1,
            }],
        );

        assert_eq!(catalog.len(), 1);
        assert!(!catalog.is_empty());
        assert_eq!(catalog.title_for_path("/missing.mra"), "Game");
        assert!(catalog.system_games("missing").is_empty());
        assert_eq!(catalog.system_game_count("missing"), 0);
        assert!(catalog.system_game_at("missing", 0).is_none());
        assert!(catalog.system_preview_games("missing").is_empty());
        assert_eq!(catalog.system_preview_game_count("missing"), 0);
        assert!(catalog.system_preview_game_at("missing", 0).is_none());
    }

    #[test]
    fn filter_options_are_precomputed_by_system() {
        let mut first = game("1942", "/games/1942.mra", "", "arcade");
        first.year = Some(1984);
        first.manufacturer = "Capcom".into();
        first.category = "Shooter - Flying Vertical".into();
        first.players = Some(2);
        first.control = "joy".into();
        let mut second = game("1943", "/games/1943.mra", "", "arcade");
        second.year = Some(1987);
        second.manufacturer = "Capcom".into();
        second.category = "Shooter - Flying Vertical".into();
        second.players = Some(2);
        second.control = "joy".into();
        let mut third = game("Out Run", "/games/outrun.mra", "", "arcade");
        third.year = Some(1986);
        third.manufacturer = "Sega".into();
        third.category = "Driving - 3rd Person".into();
        third.players = Some(1);
        third.control = "dial".into();
        let mut other_system = game("Agony", "/games/agony.mgl", "", "amiga");
        other_system.manufacturer = "Psygnosis".into();

        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            vec![first, second, third, other_system],
            Vec::new(),
        );

        assert_eq!(
            catalog.category_options("arcade"),
            vec![
                ArcadeFilterOption {
                    label: "Driving - 3rd Person".into(),
                    count: 1
                },
                ArcadeFilterOption {
                    label: "Shooter - Flying Vertical".into(),
                    count: 2
                },
            ]
        );
        assert_eq!(catalog.category_option_count("arcade"), 2);
        assert_eq!(
            catalog.decade_options("arcade"),
            vec![ArcadeFilterOption {
                label: "1980's".into(),
                count: 3
            }]
        );
        assert_eq!(catalog.decade_option_count("arcade"), 1);
        assert_eq!(
            catalog.manufacturer_options("arcade"),
            vec![
                ArcadeFilterOption {
                    label: "Capcom".into(),
                    count: 2
                },
                ArcadeFilterOption {
                    label: "Sega".into(),
                    count: 1
                },
            ]
        );
        assert_eq!(catalog.manufacturer_option_count("arcade"), 2);
        assert_eq!(
            catalog.player_options("arcade"),
            vec![
                ArcadeFilterOption {
                    label: "1 Player".into(),
                    count: 1
                },
                ArcadeFilterOption {
                    label: "2 Players".into(),
                    count: 2
                },
            ]
        );
        assert_eq!(catalog.player_option_count("arcade"), 2);
        assert_eq!(
            catalog.control_options("arcade"),
            vec![
                ArcadeFilterOption {
                    label: "Dial".into(),
                    count: 1
                },
                ArcadeFilterOption {
                    label: "Joystick".into(),
                    count: 2
                },
            ]
        );
        assert_eq!(catalog.control_option_count("arcade"), 2);
        assert_eq!(catalog.manufacturer_option_count("amiga"), 1);
        assert!(catalog.decade_options("missing").is_empty());
        assert_eq!(catalog.player_option_count("missing"), 0);
        assert_eq!(catalog.control_option_count("missing"), 0);
        assert_eq!(
            catalog.filtered_game_count(
                "arcade",
                &ArcadeFilter::Category("Shooter - Flying Vertical".into())
            ),
            2
        );
    }

    #[test]
    fn player_and_control_filters_use_mame_metadata_values() {
        let controls = ["joy", "JOY", "dial", "only_buttons", "doublejoy"];
        let games = controls
            .iter()
            .enumerate()
            .map(|(index, control)| {
                let mut entry = game(
                    &format!("Game {index}"),
                    &format!("/games/{index}.mra"),
                    "",
                    "arcade",
                );
                entry.players = Some((index % 2 + 1) as u8);
                entry.control = (*control).into();
                entry
            })
            .collect();
        let catalog = ArcadeCatalog::new(PathBuf::from("/games"), games, Vec::new());

        assert_eq!(
            catalog.control_options("arcade"),
            vec![
                ArcadeFilterOption {
                    label: "Buttons Only".into(),
                    count: 1,
                },
                ArcadeFilterOption {
                    label: "Dial".into(),
                    count: 1,
                },
                ArcadeFilterOption {
                    label: "Dual Joystick".into(),
                    count: 1,
                },
                ArcadeFilterOption {
                    label: "Joystick".into(),
                    count: 2,
                },
            ]
        );
        assert_eq!(
            catalog.filtered_game_count("arcade", &ArcadeFilter::Players(1)),
            3
        );
        assert_eq!(
            catalog.resolve_filter("arcade", &ArcadeFilter::Control("joy".to_string())),
            Some(ArcadeFilter::Control("Joystick".to_string()))
        );
        assert_eq!(
            catalog.resolve_filter("arcade", &ArcadeFilter::Control("Buttons Only".to_string())),
            Some(ArcadeFilter::Control("Buttons Only".to_string()))
        );
    }

    #[test]
    fn arcade_search_matches_metadata_and_ranks_titles_first() {
        let mut capcom_shooter = game("1942", "/games/1942.mra", "", "arcade");
        capcom_shooter.year = Some(1984);
        capcom_shooter.manufacturer = "Capcom".into();
        capcom_shooter.control = "lightgun".into();
        let mut capcom_fighter = game("Final Fight", "/games/ffight.mra", "", "arcade");
        capcom_fighter.year = Some(1989);
        capcom_fighter.manufacturer = "Capcom".into();
        capcom_fighter.control = "doublejoy".into();
        let mut title_match = game("Capcom Sports Club", "/games/csclub.mra", "", "arcade");
        title_match.manufacturer = "Mitchell".into();
        title_match.control = "only_buttons".into();
        let mut other_system = game("Capcom Quiz", "/games/capquiz.mgl", "", "amiga");
        other_system.manufacturer = "Capcom".into();

        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            vec![capcom_shooter, capcom_fighter, title_match, other_system],
            Vec::new(),
        );

        let capcom = catalog.search_game_indexes("arcade", "capcom");
        assert_eq!(capcom, vec![2, 0, 1]);

        let capcom_fighters = catalog.search_game_indexes("arcade", "capcom dual");
        assert_eq!(capcom_fighters, vec![1]);

        let vertical = catalog.search_game_indexes("arcade", "light gun");
        assert_eq!(vertical, vec![0]);

        let year = catalog.search_game_indexes("arcade", "1984");
        assert_eq!(year, vec![0]);
    }

    #[test]
    fn navigation_projection_catalog_defers_text_indexes_without_changing_search() {
        let mut capcom_shooter = game("1942", "/games/1942.mra", "", "arcade");
        capcom_shooter.year = Some(1984);
        capcom_shooter.manufacturer = "Capcom".into();
        capcom_shooter.control = "lightgun".into();
        let mut capcom_fighter = game("Final Fight", "/games/ffight.mra", "", "arcade");
        capcom_fighter.year = Some(1989);
        capcom_fighter.manufacturer = "Capcom".into();
        capcom_fighter.control = "doublejoy".into();
        let mut title_match = game("Capcom Sports Club", "/games/csclub.mra", "", "arcade");
        title_match.manufacturer = "Mitchell".into();
        title_match.control = "only_buttons".into();
        let mut other_system = game("Capcom Quiz", "/games/capquiz.mgl", "", "amiga");
        other_system.manufacturer = "Capcom".into();
        let games = vec![capcom_shooter, capcom_fighter, title_match, other_system];
        let systems = vec![
            GameSystemEntry {
                id: "arcade".into(),
                title: "Arcade".into(),
                count: 3,
            },
            GameSystemEntry {
                id: "amiga".into(),
                title: "Amiga".into(),
                count: 1,
            },
        ];
        let eager = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            games.clone(),
            systems.clone(),
        );
        let projection = CatalogNavigationProjection::from_catalog(
            &ArcadeCatalog::new(PathBuf::from("/media/fat/_Arcade"), games, systems),
            &crate::catalog_stamp::CatalogStamp::from_lines(vec!["arcade|1|2".into()]),
        );
        let deferred = ArcadeCatalog::from_navigation_projection("/media/fat/_Arcade", projection);

        assert!(deferred.search_keys.is_empty());
        assert!(deferred.lazy_text_indexes.get().is_none());
        for query in [
            "capcom",
            "capcom dual",
            "light gun",
            "1984",
            "csclub",
            "missing",
        ] {
            assert_eq!(
                deferred.search_game_indexes("arcade", query),
                eager.search_game_indexes("arcade", query),
                "query {query}"
            );
        }
        assert!(deferred.lazy_text_indexes.get().is_some());
    }

    #[test]
    fn navigation_projection_catalog_defers_text_indexes_without_changing_autocomplete() {
        let mut street = game("Street Fighter II", "/games/sf2.mra", "", "arcade");
        street.manufacturer = "Capcom".into();
        street.control = "doublejoy".into();
        let mut pac = game("Pac-Man", "/games/pacman.mra", "", "arcade");
        pac.manufacturer = "Namco".into();
        pac.control = "trackball".into();
        let mut shooter = game("1942", "/games/1942.mra", "", "arcade");
        shooter.year = Some(1984);
        shooter.manufacturer = "Capcom".into();
        shooter.control = "lightgun".into();
        let mut other_system = game("Street Racer", "/games/street-racer.mgl", "", "amiga");
        other_system.manufacturer = "Psygnosis".into();
        let games = vec![street, pac, shooter, other_system];
        let systems = vec![
            GameSystemEntry {
                id: "arcade".into(),
                title: "Arcade".into(),
                count: 3,
            },
            GameSystemEntry {
                id: "amiga".into(),
                title: "Amiga".into(),
                count: 1,
            },
        ];
        let eager = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            games.clone(),
            systems.clone(),
        );
        let projection = CatalogNavigationProjection::from_catalog(
            &ArcadeCatalog::new(PathBuf::from("/media/fat/_Arcade"), games, systems),
            &crate::catalog_stamp::CatalogStamp::from_lines(vec!["arcade|1|2".into()]),
        );
        let deferred = ArcadeCatalog::from_navigation_projection("/media/fat/_Arcade", projection);

        assert!(deferred.search_keys.is_empty());
        assert!(deferred.lazy_text_indexes.get().is_none());
        for query in ["str", "fig", "pac", "cap", "lig", "194", "psy", "x"] {
            assert_eq!(
                deferred.autocomplete_search_word("arcade", query),
                eager.autocomplete_search_word("arcade", query),
                "query {query}"
            );
        }
        assert!(deferred.lazy_text_indexes.get().is_some());
    }

    #[test]
    fn deferred_text_constructor_keeps_search_and_autocomplete_equivalent() {
        let mut street = game("Street Fighter II", "/games/sf2.mra", "", "arcade");
        street.manufacturer = "Capcom".into();
        street.control = "doublejoy".into();
        let mut pac = game("Pac-Man", "/games/pacman.mra", "", "arcade");
        pac.manufacturer = "Namco".into();
        pac.control = "trackball".into();
        let mut shooter = game("1942", "/games/1942.mra", "", "arcade");
        shooter.year = Some(1984);
        shooter.manufacturer = "Capcom".into();
        shooter.control = "lightgun".into();
        let games = vec![street, pac, shooter];
        let systems = vec![GameSystemEntry {
            id: "arcade".into(),
            title: "Arcade".into(),
            count: 3,
        }];
        let eager = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            games.clone(),
            systems.clone(),
        );
        let deferred = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from("/media/fat/_Arcade"),
            games,
            systems,
            Vec::new(),
        );

        assert!(deferred.search_keys.is_empty());
        assert!(deferred.lazy_text_indexes.get().is_none());
        for query in ["street", "capcom", "fighter", "1984", "missing"] {
            assert_eq!(
                deferred.search_game_indexes("arcade", query),
                eager.search_game_indexes("arcade", query),
                "query {query}"
            );
        }
        for query in ["str", "fig", "pac", "cap", "lig", "194", "x"] {
            assert_eq!(
                deferred.autocomplete_search_word("arcade", query),
                eager.autocomplete_search_word("arcade", query),
                "query {query}"
            );
        }
        assert!(deferred.lazy_text_indexes.get().is_some());
    }

    #[test]
    fn deferred_text_indexes_build_once_on_first_text_access() {
        let mut street = game("Street Fighter II", "/games/sf2.mra", "", "arcade");
        street.manufacturer = "Capcom".into();
        street.control = "doublejoy".into();
        let mut shooter = game("1942", "/games/1942.mra", "", "arcade");
        shooter.year = Some(1984);
        let games = vec![street, shooter];
        let systems = vec![GameSystemEntry {
            id: "arcade".into(),
            title: "Arcade".into(),
            count: 2,
        }];

        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from("/media/fat/_Arcade"),
            games,
            systems,
            Vec::new(),
        );

        assert!(catalog.lazy_text_indexes.get().is_none());
        assert_eq!(catalog.system_game_count("arcade"), 2);
        assert_eq!(catalog.filtered_game_count("arcade", &ArcadeFilter::All), 2);
        assert!(catalog.lazy_text_indexes.get().is_none());

        assert_eq!(catalog.search_game_indexes("arcade", "capcom"), vec![0]);
        let built = catalog
            .lazy_text_indexes
            .get()
            .expect("search should build text indexes")
            as *const ArcadeTextIndexes;
        assert_eq!(catalog.autocomplete_search_word("arcade", "19"), "1942");
        let reused = catalog
            .lazy_text_indexes
            .get()
            .expect("autocomplete should reuse text indexes")
            as *const ArcadeTextIndexes;
        assert_eq!(built, reused);
    }

    #[test]
    fn deferred_text_index_job_can_cancel_before_publishing_a_generation() {
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from("/media/fat/_Arcade"),
            vec![game("Street Fighter II", "/games/sf2.mra", "", "arcade")],
            vec![GameSystemEntry {
                id: "arcade".into(),
                title: "Arcade".into(),
                count: 1,
            }],
            Vec::new(),
        );
        let job = catalog.text_index_build_job().expect("deferred job");
        assert!(job.build_with_timing_while(|| false).is_none());
        assert!(!catalog.text_indexes_ready());
    }

    #[test]
    fn deferred_text_indexes_can_be_prewarmed_explicitly() {
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from("/media/fat/_Arcade"),
            vec![game("Street Fighter II", "/games/sf2.mra", "", "arcade")],
            vec![GameSystemEntry {
                id: "arcade".into(),
                title: "Arcade".into(),
                count: 1,
            }],
            Vec::new(),
        );

        assert!(!catalog.text_indexes_ready());
        assert!(catalog.ensure_text_indexes_ready());
        assert!(catalog.text_indexes_ready());
        assert!(!catalog.ensure_text_indexes_ready());
    }

    #[test]
    fn arcade_search_autocomplete_prefers_current_system_metadata_and_titles() {
        let mut street = game("Street Fighter II", "/games/sf2.mra", "", "arcade");
        street.manufacturer = "Capcom".into();
        street.control = "doublejoy".into();
        let mut pac = game("Pac-Man", "/games/pacman.mra", "", "arcade");
        pac.manufacturer = "Namco".into();
        pac.control = "trackball".into();
        let mut shooter = game("1942", "/games/1942.mra", "", "arcade");
        shooter.year = Some(1984);
        shooter.manufacturer = "Capcom".into();
        shooter.control = "lightgun".into();
        let mut other_system = game("Street Racer", "/games/street-racer.mgl", "", "amiga");
        other_system.manufacturer = "Psygnosis".into();

        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            vec![street, pac, shooter, other_system],
            Vec::new(),
        );

        assert_eq!(catalog.autocomplete_search_word("arcade", "str"), "street");
        assert_eq!(catalog.autocomplete_search_word("arcade", "fig"), "fighter");
        assert_eq!(catalog.autocomplete_search_word("arcade", "pac"), "pac");
        assert_eq!(catalog.autocomplete_search_word("arcade", "cap"), "capcom");
        assert_eq!(catalog.autocomplete_search_word("arcade", "lig"), "light");
        assert_eq!(catalog.autocomplete_search_word("arcade", "194"), "1942");
        assert_eq!(catalog.autocomplete_search_word("arcade", "x"), "");
    }

    #[test]
    fn arcade_search_autocomplete_prefers_current_system_visible_words_before_path_noise() {
        let mut path_noise = Vec::new();
        for index in 0..20 {
            path_noise.push(game(
                &format!("Other Game {index}"),
                &format!("/games/cap-path-noise-{index}.mgl"),
                "",
                "amiga",
            ));
        }
        let mut capcom = game("1942", "/games/1942.mra", "", "arcade");
        capcom.manufacturer = "Capcom".into();
        let mut catalog_games = vec![capcom];
        catalog_games.extend(path_noise);

        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            catalog_games,
            Vec::new(),
        );

        assert_eq!(catalog.autocomplete_search_word("arcade", "cap"), "capcom");
    }

    #[test]
    fn arcade_search_real_fixture_expectations_when_available() {
        let Some(catalog) = optional_real_fixture_catalog() else {
            return;
        };

        assert!(catalog.search_game_indexes("arcade", "capcom").len() >= 50);
        assert!(
            !catalog
                .search_game_indexes("arcade", "street fighter")
                .is_empty()
        );
        assert!(!catalog.search_game_indexes("arcade", "pac man").is_empty());
        assert!(!catalog.search_game_indexes("arcade", "194").is_empty());

        assert_eq!(catalog.autocomplete_search_word("arcade", "cap"), "capcom");
        assert_eq!(catalog.autocomplete_search_word("arcade", "seg"), "sega");
        assert_eq!(catalog.autocomplete_search_word("arcade", "kon"), "konami");
        assert_eq!(catalog.autocomplete_search_word("arcade", "str"), "street");
        assert_eq!(catalog.autocomplete_search_word("arcade", "fig"), "fighter");
        assert_eq!(catalog.autocomplete_search_word("arcade", "pac"), "pac");
    }

    #[cfg(test)]
    fn optional_real_fixture_catalog() -> Option<ArcadeCatalog> {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("private/test-fixtures/autocomplete-launcher-catalog.tsv");
        let contents = std::fs::read_to_string(fixture).ok()?;
        let mut games = Vec::new();
        for line in contents.lines() {
            if line.starts_with("system_id\t") || line.starts_with("library_sql_timing_tsv") {
                continue;
            }
            let columns = line.split('\t').collect::<Vec<_>>();
            if columns.len() < 6 {
                continue;
            }
            let year = columns[3].trim().parse::<u16>().ok();
            let players = columns
                .get(5)
                .and_then(|value| value.trim().parse::<u8>().ok())
                .filter(|players| *players > 0);
            let control = columns.get(6).copied().unwrap_or_default();
            games.push(ArcadeGameEntry {
                system_id: columns[0].into(),
                title: columns[1].into(),
                mra_path: columns[2].into(),
                year,
                manufacturer: columns[4].into(),
                category: "".into(),
                players,
                control: control.into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                is_new: false,
            });
        }
        let systems = systems_from_games(&games);
        Some(ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            games,
            systems,
        ))
    }

    #[test]
    fn preview_games_prefer_exact_or_shorter_title_for_same_family() {
        let games = [
            game(
                "Puzzle Star (World)",
                "/games/puzzle-world.mra",
                "puzzle-world",
                "arcade",
            ),
            game("Puzzle Star", "/games/puzzle.mra", "puzzle", "arcade"),
            game(
                "Space   Duel Alpha",
                "/games/space-extended.mra",
                "space-extended",
                "arcade",
            ),
            game("Space Duel Alpha", "/games/space.mra", "space", "arcade"),
        ];

        let previews = preview_games(&games, 0..games.len());

        assert_eq!(
            previews
                .iter()
                .map(|index| games[*index].title.as_ref())
                .collect::<Vec<_>>(),
            vec!["Puzzle Star", "Space Duel Alpha"]
        );
    }

    #[test]
    fn preview_games_require_preview_archive_and_asset_key() {
        let games = [
            game("Still Image", "/games/still.mra", "", "arcade"),
            game("Photo", "/games/photo.mra", "photo", "arcade"),
        ];

        let previews = preview_games(&games, 0..games.len());

        assert_eq!(previews.len(), 1);
        assert_eq!(games[previews[0]].title.as_ref(), "Photo");
    }

    #[test]
    fn arcade_catalog_sorts_systems_by_human_title() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            Vec::new(),
            vec![
                GameSystemEntry {
                    id: "neogeo".into(),
                    title: "NeoGeo".into(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "amiga".into(),
                    title: "Amiga".into(),
                    count: 1,
                },
            ],
        );

        assert_eq!(
            catalog
                .systems
                .iter()
                .map(|system| system.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Amiga", "Arcade", "NeoGeo"]
        );
    }

    #[test]
    fn systems_from_games_uses_alphabetical_human_titles() {
        let games = vec![
            game(
                "Unknown Thing",
                "/media/fat/_Arcade/Unknown.mra",
                "",
                "unknown",
            ),
            game(
                "Metal Slug",
                "/media/fat/_Arcade/Metal Slug.mra",
                "",
                "neogeo",
            ),
            game(
                "Sonic",
                "/media/fat/games/MegaDrive/Sonic.md",
                "",
                "megadrive",
            ),
            game("1942", "/media/fat/_Arcade/1942.mra", "", "arcade"),
            game(
                "Another Sonic",
                "/media/fat/games/MegaDrive/Another Sonic.md",
                "",
                "megadrive",
            ),
            game("Workbench", "/media/fat/_Computer/Amiga.mgl", "", "amiga"),
        ];

        let systems = systems_from_games(&games);

        assert_eq!(
            systems,
            vec![
                GameSystemEntry {
                    id: "amiga".into(),
                    title: "Amiga".into(),
                    count: 1
                },
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 1
                },
                GameSystemEntry {
                    id: "megadrive".into(),
                    title: "Mega Drive".into(),
                    count: 2
                },
                GameSystemEntry {
                    id: "neogeo".into(),
                    title: "NeoGeo".into(),
                    count: 1
                },
                GameSystemEntry {
                    id: "unknown".into(),
                    title: "Unknown".into(),
                    count: 1
                }
            ]
        );
    }

    #[test]
    fn platform_kind_normalizes_profile_categories_and_infers_fixture_systems() {
        assert_eq!(
            PlatformKind::from_category(" Arcade "),
            PlatformKind::Arcade
        );
        assert_eq!(
            PlatformKind::from_category("CONSOLE"),
            PlatformKind::Console
        );
        assert_eq!(
            PlatformKind::from_category("Handheld"),
            PlatformKind::Handheld
        );
        assert_eq!(
            PlatformKind::from_category("computer"),
            PlatformKind::Computer
        );
        assert_eq!(
            PlatformKind::from_category("Launcher"),
            PlatformKind::Unknown
        );
        assert_eq!(
            PlatformKind::inferred_for_system_id("wonderswan"),
            PlatformKind::Handheld
        );
        assert_eq!(
            PlatformKind::inferred_for_system_id("cps1"),
            PlatformKind::Arcade
        );
        assert_eq!(
            PlatformKind::inferred_for_system_id("mystery"),
            PlatformKind::Unknown
        );
    }

    #[test]
    fn virtual_arcade_collection_uses_prebuilt_catalog_indexes() {
        let mut snk_arcade = game(
            "Metal Slug Arcade",
            "/media/fat/_Arcade/Metal Slug Arcade.mra",
            "metal-slug",
            "arcade",
        );
        snk_arcade.manufacturer = "SNK (Rock-Ola license)".into();
        snk_arcade.control = "Run and Gun".into();
        snk_arcade.year = Some(1996);

        let mut capcom_arcade = game(
            "Cyberbots",
            "/media/fat/_Arcade/Cyberbots.mra",
            "cyberbots",
            "cps2",
        );
        capcom_arcade.manufacturer = "Capcom".into();
        capcom_arcade.control = "Fighter".into();
        capcom_arcade.year = Some(1995);

        let mut snk_cps = game("P.O.W.", "/media/fat/_Arcade/P.O.W..mra", "pow", "cps1");
        snk_cps.manufacturer = "SNK".into();
        snk_cps.control = "Beat 'em up".into();
        snk_cps.year = Some(1988);

        let mut neogeo = game(
            "Metal Slug NeoGeo",
            "/media/fat/games/NeoGeo/Metal Slug.neo",
            "metal-slug-neogeo",
            "neogeo",
        );
        neogeo.manufacturer = "SNK".into();
        neogeo.control = "Run and Gun".into();
        neogeo.year = Some(1996);

        let mut saturn = game(
            "Fighters Megamix",
            "/media/fat/games/Saturn/Fighters Megamix.chd",
            "fighters-megamix",
            "saturn",
        );
        saturn.manufacturer = "Sega".into();
        saturn.control = "Fighter".into();
        saturn.year = Some(1996);

        let sms = game(
            "Sonic the Hedgehog",
            "/media/fat/games/SMS/Sonic.sms",
            "sonic-sms",
            "sms",
        );
        let gamegear = game(
            "Columns",
            "/media/fat/games/GameGear/Columns.gg",
            "columns-gamegear",
            "gamegear",
        );
        let astrocade = game(
            "Wizard of Wor",
            "/media/fat/games/Astrocade/Wizard of Wor.zip",
            "wizard-of-wor-astrocade",
            "astrocade",
        );

        let games = vec![
            snk_arcade,
            capcom_arcade,
            snk_cps,
            neogeo,
            saturn,
            sms,
            gamegear,
            astrocade,
        ];
        let systems = systems_from_games(&games);
        let catalog = ArcadeCatalog::new(PathBuf::from(DEFAULT_ARCADE_ROOT), games, systems);

        assert_eq!(catalog.platform_kind("cps2"), PlatformKind::Arcade);
        assert_eq!(catalog.platform_kind("saturn"), PlatformKind::Console);
        assert_eq!(catalog.platform_kind("sms"), PlatformKind::Console);
        assert_eq!(catalog.platform_kind("gamegear"), PlatformKind::Handheld);
        assert_eq!(catalog.platform_kind("astrocade"), PlatformKind::Console);
        assert_eq!(catalog.system_game_count(MENU_ARCADE_SYSTEM_ID), 3);
        assert_eq!(
            catalog.system_game_count(MENU_ARCADE_SYSTEM_ID),
            catalog.system_game_view(MENU_ARCADE_SYSTEM_ID).len(),
            "the Arcade number is exactly its visible, parent-collapsed projection"
        );
        assert_eq!(
            catalog
                .system_game_view(MENU_ARCADE_SYSTEM_ID)
                .iter()
                .map(|game| game.title.as_ref())
                .collect::<Vec<_>>(),
            vec!["Metal Slug Arcade", "Cyberbots", "P.O.W."]
        );
        assert_eq!(
            catalog.filtered_game_count(MENU_ARCADE_SYSTEM_ID, &ArcadeFilter::Decade(1990)),
            2
        );
        assert_eq!(catalog.system_preview_game_count(MENU_ARCADE_SYSTEM_ID), 3);
        assert_eq!(
            catalog.search_game_indexes(MENU_ARCADE_SYSTEM_ID, "cyberbots"),
            vec![1]
        );
        assert!(
            catalog
                .autocomplete
                .words
                .get("cyberbots")
                .is_some_and(|stats| stats.system_scores.contains_key(MENU_ARCADE_SYSTEM_ID))
        );
    }
}
