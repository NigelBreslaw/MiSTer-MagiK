//! Arcade catalog helpers.
//!
//! The runtime launcher catalog is SQLite-backed. This module keeps the shared
//! in-memory catalog types and presentation helpers used by the SQLite loader.

use crate::launch_profiles::{self, MountKind, PayloadRule};
use crate::catalog_navigation::CatalogNavigationProjection;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

pub const DEFAULT_ARCADE_ROOT: &str = "/media/fat/_Arcade";

/// Logical row height for the Rust-painted arcade list viewport.
pub const ARCADE_ROW_HEIGHT: i32 = 48;
/// Visible list height: 8 exact arcade rows (matches `arcade_list.slint` left pane).
pub const ARCADE_LIST_VISIBLE_H: i32 = 384;
pub const HOME_TILE_WIDTH: i32 = 220;
pub const HOME_TILE_GAP: i32 = 16;
pub const HOME_LIST_VISIBLE_W: i32 = 912;

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
    pub is_new: bool,
}

impl ArcadeGameEntry {
    pub fn metadata_key(&self) -> ArcadeGameMetadataKey {
        ArcadeGameMetadataKey {
            year: self.year,
            manufacturer: self.manufacturer.to_string(),
            category: self.category.to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArcadeGameMetadataKey {
    pub year: Option<u16>,
    pub manufacturer: String,
    pub category: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameSystemEntry {
    pub id: String,
    pub title: String,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ArcadeFilter {
    All,
    Search,
    Decade(u16),
    Manufacturer(String),
    Category(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArcadeFilterOption {
    pub label: String,
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct ArcadeCatalog {
    pub root: PathBuf,
    pub games: Vec<ArcadeGameEntry>,
    pub systems: Vec<GameSystemEntry>,
    games_by_system: HashMap<String, Vec<usize>>,
    games_by_filter: HashMap<ArcadeFilterKey, Vec<usize>>,
    filter_options_by_system: HashMap<String, ArcadeSystemFilterOptions>,
    preview_games_by_system: HashMap<String, Vec<usize>>,
    games_by_ref: HashMap<Arc<str>, usize>,
    launch_plans_by_ref: HashMap<Arc<str>, StructuredLaunchPlan>,
    search_keys: Vec<ArcadeSearchKey>,
    autocomplete: ArcadeAutocompleteIndex,
    lazy_text_indexes: OnceLock<ArcadeTextIndexes>,
}

#[derive(Clone, Copy, Debug)]
pub enum ArcadeGameView<'a> {
    Contiguous(&'a [ArcadeGameEntry]),
    Indexed {
        games: &'a [ArcadeGameEntry],
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

    pub fn len(self) -> usize {
        match self {
            Self::Contiguous(games) => games.len(),
            Self::Indexed { indexes, .. } => indexes.len(),
        }
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn get(self, index: usize) -> Option<&'a ArcadeGameEntry> {
        match self {
            Self::Contiguous(games) => games.get(index),
            Self::Indexed { games, indexes } => {
                indexes.get(index).and_then(|game_index| games.get(*game_index))
            }
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
    decades: Vec<ArcadeFilterOption>,
    manufacturers: Vec<ArcadeFilterOption>,
    categories: Vec<ArcadeFilterOption>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct ArcadeFilterKey {
    system_id: Arc<str>,
    kind: ArcadeFilterKindKey,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum ArcadeFilterKindKey {
    Decade(u16),
    Manufacturer(Arc<str>),
    Category(Arc<str>),
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
pub enum LaunchTarget {
    Path(Arc<str>),
    Structured(StructuredLaunchPlan),
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

    fn new_with_launch_plans_and_index_mode(
        root: PathBuf,
        games: Vec<ArcadeGameEntry>,
        systems: Vec<GameSystemEntry>,
        launch_plans: Vec<StructuredLaunchPlan>,
        index_mode: CatalogIndexMode,
    ) -> Self {
        let indexes = build_arcade_catalog_indexes(&games, launch_plans, index_mode);
        Self {
            root,
            games,
            systems,
            games_by_system: indexes.games_by_system,
            games_by_filter: indexes.games_by_filter,
            filter_options_by_system: indexes.filter_options_by_system,
            preview_games_by_system: indexes.preview_games_by_system,
            games_by_ref: indexes.games_by_ref,
            launch_plans_by_ref: indexes.launch_plans_by_ref,
            search_keys: indexes.search_keys,
            autocomplete: indexes.autocomplete,
            lazy_text_indexes: OnceLock::new(),
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
        let indexes = build_arcade_catalog_indexes(&games, launch_plans, index_mode);
        Self {
            root: root.into(),
            games,
            systems,
            games_by_system: indexes.games_by_system,
            games_by_filter: indexes.games_by_filter,
            filter_options_by_system: indexes.filter_options_by_system,
            preview_games_by_system: indexes.preview_games_by_system,
            games_by_ref: indexes.games_by_ref,
            launch_plans_by_ref: indexes.launch_plans_by_ref,
            search_keys: indexes.search_keys,
            autocomplete: indexes.autocomplete,
            lazy_text_indexes: OnceLock::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    pub fn title_for_path(&self, mra_path: &str) -> &str {
        self.games
            .iter()
            .find(|g| g.mra_path.as_ref() == mra_path)
            .map(|g| g.title.as_ref())
            .unwrap_or("Game")
    }

    pub fn launch_target_for_ref(&self, launch_ref: &str) -> LaunchTarget {
        self.launch_plans_by_ref
            .get(launch_ref)
            .cloned()
            .map(LaunchTarget::Structured)
            .unwrap_or_else(|| {
                if launch_ref.starts_with("magik-plan:") {
                    self.games_by_ref
                        .get(launch_ref)
                        .and_then(|idx| self.games.get(*idx))
                        .and_then(|game| derive_structured_launch_plan(game, profiles_by_system()))
                        .map(LaunchTarget::Structured)
                        .unwrap_or_else(|| LaunchTarget::MissingStructured(Arc::from(launch_ref)))
                } else {
                    LaunchTarget::Path(Arc::from(launch_ref))
                }
            })
    }

    pub fn system_games(&self, system_id: &str) -> Vec<ArcadeGameEntry> {
        self.system_game_view(system_id).iter().cloned().collect()
    }

    pub fn system_game_count(&self, system_id: &str) -> usize {
        self.system_game_indexes(system_id).len()
    }

    pub fn system_game_at(&self, system_id: &str, index: usize) -> Option<&ArcadeGameEntry> {
        self.system_game_view(system_id).get(index)
    }

    pub fn system_game_view(&self, system_id: &str) -> ArcadeGameView<'_> {
        ArcadeGameView::indexed(&self.games, self.system_game_indexes(system_id))
    }

    pub fn search_game_indexes(&self, system_id: &str, query: &str) -> Vec<usize> {
        let needle = search_match_key(query);
        let compact_needle = compact_search_match_key(query);
        let tokens = search_query_tokens(&needle);
        if needle.is_empty() && compact_needle.is_empty() {
            return self.system_game_indexes(system_id).to_vec();
        }
        let mut scored: Vec<SearchMatch> = self
            .system_game_indexes(system_id)
            .iter()
            .copied()
            .filter_map(|index| {
                let score = self
                    .search_keys()
                    .get(index)
                    .and_then(|key| key.score(&tokens, &compact_needle))?;
                Some(SearchMatch { index, score })
            })
            .collect();
        scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.index.cmp(&b.index)));
        scored.into_iter().map(|entry| entry.index).collect()
    }

    pub fn autocomplete_search_word(&self, system_id: &str, query: &str) -> String {
        let fragment = current_search_word(query);
        self.autocomplete()
            .suggest(system_id, fragment)
            .unwrap_or_default()
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
            .get_or_init(|| build_arcade_text_indexes(&self.games))
    }

    pub fn filtered_game_count(&self, system_id: &str, filter: &ArcadeFilter) -> usize {
        match filter {
            ArcadeFilter::All => self.system_game_count(system_id),
            ArcadeFilter::Search => self.system_game_count(system_id),
            _ => self.filtered_game_indexes(system_id, filter).len(),
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
            _ => self
                .filtered_game_indexes(system_id, filter)
                .get(index)
                .and_then(|game_index| self.games.get(*game_index)),
        }
    }

    pub fn filtered_game_view(
        &self,
        system_id: &str,
        filter: &ArcadeFilter,
    ) -> ArcadeGameView<'_> {
        match filter {
            ArcadeFilter::All => self.system_game_view(system_id),
            ArcadeFilter::Search => self.system_game_view(system_id),
            _ => ArcadeGameView::indexed(&self.games, self.filtered_game_indexes(system_id, filter)),
        }
    }

    pub fn decade_options(&self, system_id: &str) -> Vec<ArcadeFilterOption> {
        self.filter_options(system_id).decades.clone()
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

    pub fn category_options(&self, system_id: &str) -> Vec<ArcadeFilterOption> {
        self.filter_options(system_id).categories.clone()
    }

    pub fn category_option_count(&self, system_id: &str) -> usize {
        self.filter_options(system_id).categories.len()
    }

    pub fn system_preview_games(&self, system_id: &str) -> Vec<ArcadeGameEntry> {
        self.preview_game_indexes(system_id)
            .iter()
            .filter_map(|index| self.games.get(*index).cloned())
            .collect()
    }

    pub fn system_preview_game_count(&self, system_id: &str) -> usize {
        self.preview_game_indexes(system_id).len()
    }

    pub fn system_preview_game_at(&self, system_id: &str, index: usize) -> Option<ArcadeGameEntry> {
        self.preview_game_indexes(system_id)
            .get(index)
            .and_then(|game_index| self.games.get(*game_index))
            .cloned()
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
        self.filter_options_by_system
            .get(system_id)
            .unwrap_or_else(|| EMPTY.get_or_init(ArcadeSystemFilterOptions::default))
    }
}

struct ArcadeCatalogIndexes {
    games_by_system: HashMap<String, Vec<usize>>,
    games_by_filter: HashMap<ArcadeFilterKey, Vec<usize>>,
    filter_options_by_system: HashMap<String, ArcadeSystemFilterOptions>,
    preview_games_by_system: HashMap<String, Vec<usize>>,
    games_by_ref: HashMap<Arc<str>, usize>,
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

#[derive(Clone, Debug)]
struct ArcadeSearchKey {
    title: String,
    path: String,
    manufacturer: String,
    category: String,
    year: String,
    decade: String,
    compact_title: String,
    compact_path: String,
    compact_manufacturer: String,
    compact_category: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SearchMatch {
    index: usize,
    score: u16,
}

fn build_arcade_catalog_indexes(
    games: &[ArcadeGameEntry],
    launch_plans: Vec<StructuredLaunchPlan>,
    mode: CatalogIndexMode,
) -> ArcadeCatalogIndexes {
    let mut games_by_system: HashMap<String, Vec<usize>> = HashMap::new();
    let mut games_by_filter: HashMap<ArcadeFilterKey, Vec<usize>> = HashMap::new();
    let mut filter_counts_by_system = HashMap::<String, FilterOptionCounts>::new();
    let mut preview_games_by_system: HashMap<String, Vec<usize>> = HashMap::new();
    let mut preview_best_by_system = HashMap::<String, HashMap<String, usize>>::new();
    let mut games_by_ref: HashMap<Arc<str>, usize> = HashMap::with_capacity(games.len());
    let mut text_indexes = match mode {
        CatalogIndexMode::Eager => build_arcade_text_indexes(games),
        CatalogIndexMode::DeferredText => ArcadeTextIndexes::default(),
    };

    for (idx, game) in games.iter().enumerate() {
        let system_id_string = game.system_id.to_string();
        let system_id_arc = game.system_id.clone();
        games_by_ref.insert(game.mra_path.clone(), idx);
        games_by_system
            .entry(system_id_string.clone())
            .or_default()
            .push(idx);

        let counts = filter_counts_by_system
            .entry(system_id_string.clone())
            .or_default();
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

        if !game.category.is_empty() {
            games_by_filter
                .entry(ArcadeFilterKey {
                    system_id: system_id_arc,
                    kind: ArcadeFilterKindKey::Category(game.category.clone()),
                })
                .or_default()
                .push(idx);
        }
        let category = game.category.trim();
        if !category.is_empty() {
            *counts.categories.entry(category.to_string()).or_default() += 1;
        }

        if has_preview_image(game) {
            let preview_indexes = preview_games_by_system
                .entry(system_id_string.clone())
                .or_default();
            let best_by_key = preview_best_by_system
                .entry(system_id_string)
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

    let filter_options_by_system = filter_counts_by_system
        .into_iter()
        .map(|(system_id, counts)| {
            (
                system_id,
                ArcadeSystemFilterOptions {
                    decades: counts
                        .decades
                        .into_iter()
                        .map(|(decade, count)| ArcadeFilterOption {
                            label: format!("{decade}'s"),
                            count,
                        })
                        .collect(),
                    manufacturers: string_filter_options_from_counts(counts.manufacturers),
                    categories: string_filter_options_from_counts(counts.categories),
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
        games_by_ref,
        launch_plans_by_ref,
        search_keys: std::mem::take(&mut text_indexes.search_keys),
        autocomplete: std::mem::take(&mut text_indexes.autocomplete),
    }
}

fn build_arcade_text_indexes(games: &[ArcadeGameEntry]) -> ArcadeTextIndexes {
    let mut search_keys = Vec::with_capacity(games.len());
    let mut autocomplete = ArcadeAutocompleteIndex::default();
    for game in games {
        search_keys.push(ArcadeSearchKey::from_game(game));
        autocomplete.add_game(game);
    }
    ArcadeTextIndexes {
        search_keys,
        autocomplete,
    }
}

impl ArcadeSearchKey {
    fn from_game(game: &ArcadeGameEntry) -> Self {
        let title = search_match_key(&game.title);
        let path = search_match_key(mra_basename(&game.mra_path));
        let manufacturer = search_match_key(&game.manufacturer);
        let category = search_match_key(&game.category);
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
            title,
            path,
            manufacturer,
            category,
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
            score = score.max(search_field_score(&self.compact_category, compact_needle, 64));
            score = score.max(search_field_score(&self.compact_path, compact_needle, 35));
        }
        (score > 0).then_some(score)
    }

    fn score_token(&self, token: &str) -> u16 {
        search_field_score(&self.title, token, 100)
            .max(search_field_score(&self.manufacturer, token, 80))
            .max(search_field_score(&self.category, token, 70))
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

    fn add_game(&mut self, game: &ArcadeGameEntry) {
        self.add_words(&game.system_id, &game.title, AutocompleteSource::Title);
        self.add_words(&game.system_id, &game.manufacturer, AutocompleteSource::Metadata);
        self.add_words(&game.system_id, &game.category, AutocompleteSource::Metadata);
        self.add_words(
            &game.system_id,
            mra_basename(&game.mra_path),
            AutocompleteSource::Path,
        );
        if let Some(year) = game.year {
            self.add_word(&game.system_id, &year.to_string(), AutocompleteSource::Metadata);
            self.add_word(
                &game.system_id,
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
                system_score: stats.system_scores.get(system_id).copied().unwrap_or_default(),
                total_score: stats.total_score,
            })
            .max()
            .map(|candidate| candidate.word)
    }
}

fn derive_structured_launch_plan(
    game: &ArcadeGameEntry,
    profiles_by_system: &HashMap<&'static str, launch_profiles::LaunchProfile>,
) -> Option<StructuredLaunchPlan> {
    let encoded_payload = game.mra_path.strip_prefix("magik-plan:")?;
    let (archive_entry, payload_path) = encoded_payload
        .strip_prefix("archive:")
        .map(|payload| (true, payload))
        .unwrap_or_else(|| {
            encoded_payload
                .strip_prefix("payload:")
                .map(|payload| (false, payload))
                .unwrap_or((false, encoded_payload))
        });
    let profile = profiles_by_system.get(game.system_id.as_ref())?;
    let core_path = profile.core_path?;
    let payload_rule = if archive_entry {
        profile.classify_archive_entry(Path::new(payload_path))
    } else {
        payload_rule_for_path(profile, payload_path)
    }?;
    Some(StructuredLaunchPlan {
        launch_ref: game.mra_path.clone(),
        title: game.title.clone(),
        system_id: game.system_id.clone(),
        core_path: core_path.into(),
        payload_path: payload_path.into(),
        mount_kind: mount_kind_label(payload_rule.mount.kind).into(),
        mount_index: payload_rule.mount.index,
        delay_secs: payload_rule.mount.delay_secs,
    })
}

fn profiles_by_system() -> &'static HashMap<&'static str, launch_profiles::LaunchProfile> {
    static PROFILES_BY_SYSTEM: OnceLock<HashMap<&'static str, launch_profiles::LaunchProfile>> =
        OnceLock::new();
    PROFILES_BY_SYSTEM.get_or_init(|| {
        launch_profiles::builtin_profiles()
            .into_iter()
            .map(|profile| (profile.system_id, profile))
            .collect()
    })
}

fn payload_rule_for_path(
    profile: &launch_profiles::LaunchProfile,
    payload_path: &str,
) -> Option<PayloadRule> {
    match profile.classify_path(Path::new(payload_path)) {
        launch_profiles::ProfilePathClass::Payload { rule } => Some(rule),
        _ => None,
    }
}

fn mount_kind_label(kind: MountKind) -> &'static str {
    match kind {
        MountKind::Launcher => "launcher",
        MountKind::LoadFile => "load-file",
        MountKind::MountImage => "mount-image",
        MountKind::Core => "core",
    }
}

fn filter_key(system_id: &str, filter: &ArcadeFilter) -> Option<ArcadeFilterKey> {
    match filter {
        ArcadeFilter::All | ArcadeFilter::Search => None,
        ArcadeFilter::Decade(decade) => Some(ArcadeFilterKey {
            system_id: Arc::from(system_id),
            kind: ArcadeFilterKindKey::Decade(*decade),
        }),
        ArcadeFilter::Manufacturer(manufacturer) => Some(ArcadeFilterKey {
            system_id: Arc::from(system_id),
            kind: ArcadeFilterKindKey::Manufacturer(Arc::from(manufacturer.as_str())),
        }),
        ArcadeFilter::Category(category) => Some(ArcadeFilterKey {
            system_id: Arc::from(system_id),
            kind: ArcadeFilterKindKey::Category(Arc::from(category.as_str())),
        }),
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
    decades: BTreeMap<u16, usize>,
    manufacturers: BTreeMap<String, usize>,
    categories: BTreeMap<String, usize>,
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
    systems.sort_by_cached_key(system_sort_key);
    systems
}

fn system_sort_key(system: &GameSystemEntry) -> String {
    let rank = match system.id.as_str() {
        "arcade" => 0,
        "amiga" => 1,
        "neogeo" => 2,
        "nes" => 3,
        "snes" => 4,
        "saturn" => 5,
        "megadrive" => 6,
        "gba" => 7,
        "gbc" => 8,
        "n64" => 9,
        "gamegear" => 10,
        "vectrex" => 11,
        "ao486" => 12,
        "dos" => 13,
        "unknown" => 999,
        _ => 100,
    };
    format!("{rank:03}-{}", system.title.to_lowercase())
}

pub fn system_title(id: &str) -> String {
    match id {
        "arcade" => "Arcade".to_string(),
        "neogeo" | "neo-geo" | "snk-neo-geo" => "NeoGeo".to_string(),
        "cps1" | "capcom-cps1" => "CPS1".to_string(),
        "cps2" | "capcom-cps2" => "CPS2".to_string(),
        "cps3" | "capcom-cps3" => "CPS3".to_string(),
        "system16" | "sega-system16" => "System 16".to_string(),
        "system18" | "sega-system18" => "System 18".to_string(),
        "m72" | "irem-m72" => "Irem M72".to_string(),
        "m92" | "irem-m92" => "Irem M92".to_string(),
        "gba" => "GBA".to_string(),
        "gbc" => "GBC".to_string(),
        "gb" => "GB".to_string(),
        "nes" => "NES".to_string(),
        "snes" => "SNES".to_string(),
        "n64" => "N64".to_string(),
        "sms" => "SMS".to_string(),
        "psx" => "PSX".to_string(),
        "ao486" => "ao486".to_string(),
        "dos" => "DOS Games".to_string(),
        "megadrive" => "Mega Drive".to_string(),
        "megacd" => "Mega CD".to_string(),
        "gamegear" => "Game Gear".to_string(),
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

    fn game(
        title: &str,
        mra_path: &str,
        preview_asset_key: &str,
        system_id: &str,
    ) -> ArcadeGameEntry {
        let has_preview = !preview_asset_key.is_empty();
        ArcadeGameEntry {
            title: title.into(),
            mra_path: mra_path.into(),
            preview_archive_path: if has_preview {
                "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b".into()
            } else {
                "".into()
            },
            preview_asset_key: preview_asset_key.into(),
            has_preview,
            system_id: system_id.into(),
            year: None,
            manufacturer: "".into(),
            category: "".into(),
            is_new: false,
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
            is_new: false,
        }];
        let catalog = ArcadeCatalog::new(root, games, systems);

        assert_eq!(catalog.system_game_count("amiga"), 1);
        assert_eq!(catalog.system_game_view("amiga").len(), 1);
        assert_eq!(catalog.system_preview_game_count("amiga"), 0);
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
        first.category = "Shooter".into();
        let mut second = game("1943", "/games/1943.mra", "", "arcade");
        second.year = Some(1987);
        second.manufacturer = "Capcom".into();
        second.category = "Shooter".into();
        let mut third = game("Out Run", "/games/outrun.mra", "", "arcade");
        third.year = Some(1986);
        third.manufacturer = "Sega".into();
        third.category = "Driving".into();
        let mut other_system = game("Agony", "/games/agony.mgl", "", "amiga");
        other_system.manufacturer = "Psygnosis".into();

        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            vec![first, second, third, other_system],
            Vec::new(),
        );

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
            catalog.category_options("arcade"),
            vec![
                ArcadeFilterOption {
                    label: "Driving".into(),
                    count: 1
                },
                ArcadeFilterOption {
                    label: "Shooter".into(),
                    count: 2
                },
            ]
        );
        assert_eq!(catalog.category_option_count("arcade"), 2);
        assert_eq!(catalog.manufacturer_option_count("amiga"), 1);
        assert!(catalog.decade_options("missing").is_empty());
        assert_eq!(catalog.category_option_count("missing"), 0);
    }

    #[test]
    fn arcade_search_matches_metadata_and_ranks_titles_first() {
        let mut capcom_shooter = game("1942", "/games/1942.mra", "", "arcade");
        capcom_shooter.year = Some(1984);
        capcom_shooter.manufacturer = "Capcom".into();
        capcom_shooter.category = "Shooter / Vertical".into();
        let mut capcom_fighter = game("Final Fight", "/games/ffight.mra", "", "arcade");
        capcom_fighter.year = Some(1989);
        capcom_fighter.manufacturer = "Capcom".into();
        capcom_fighter.category = "Fighter / 2D".into();
        let mut title_match = game("Capcom Sports Club", "/games/csclub.mra", "", "arcade");
        title_match.manufacturer = "Mitchell".into();
        title_match.category = "Sports".into();
        let mut other_system = game("Capcom Quiz", "/games/capquiz.mgl", "", "amiga");
        other_system.manufacturer = "Capcom".into();

        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            vec![capcom_shooter, capcom_fighter, title_match, other_system],
            Vec::new(),
        );

        let capcom = catalog.search_game_indexes("arcade", "capcom");
        assert_eq!(capcom, vec![2, 0, 1]);

        let capcom_fighters = catalog.search_game_indexes("arcade", "capcom fighter");
        assert_eq!(capcom_fighters, vec![1]);

        let vertical = catalog.search_game_indexes("arcade", "shooter vertical");
        assert_eq!(vertical, vec![0]);

        let year = catalog.search_game_indexes("arcade", "1984");
        assert_eq!(year, vec![0]);
    }

    #[test]
    fn navigation_projection_catalog_defers_text_indexes_without_changing_search() {
        let mut capcom_shooter = game("1942", "/games/1942.mra", "", "arcade");
        capcom_shooter.year = Some(1984);
        capcom_shooter.manufacturer = "Capcom".into();
        capcom_shooter.category = "Shooter / Vertical".into();
        let mut capcom_fighter = game("Final Fight", "/games/ffight.mra", "", "arcade");
        capcom_fighter.year = Some(1989);
        capcom_fighter.manufacturer = "Capcom".into();
        capcom_fighter.category = "Fighter / 2D".into();
        let mut title_match = game("Capcom Sports Club", "/games/csclub.mra", "", "arcade");
        title_match.manufacturer = "Mitchell".into();
        title_match.category = "Sports".into();
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
            "capcom fighter",
            "shooter vertical",
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
        street.category = "Fighter / 2D".into();
        let mut pac = game("Pac-Man", "/games/pacman.mra", "", "arcade");
        pac.manufacturer = "Namco".into();
        pac.category = "Maze / Pac-Man".into();
        let mut shooter = game("1942", "/games/1942.mra", "", "arcade");
        shooter.year = Some(1984);
        shooter.manufacturer = "Capcom".into();
        shooter.category = "Shooter / Vertical".into();
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
        for query in ["str", "fig", "pac", "cap", "sho", "194", "psy", "x"] {
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
        street.category = "Fighter / 2D".into();
        let mut pac = game("Pac-Man", "/games/pacman.mra", "", "arcade");
        pac.manufacturer = "Namco".into();
        pac.category = "Maze / Pac-Man".into();
        let mut shooter = game("1942", "/games/1942.mra", "", "arcade");
        shooter.year = Some(1984);
        shooter.manufacturer = "Capcom".into();
        shooter.category = "Shooter / Vertical".into();
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
        for query in ["str", "fig", "pac", "cap", "sho", "194", "x"] {
            assert_eq!(
                deferred.autocomplete_search_word("arcade", query),
                eager.autocomplete_search_word("arcade", query),
                "query {query}"
            );
        }
        assert!(deferred.lazy_text_indexes.get().is_some());
    }

    #[test]
    fn arcade_search_autocomplete_prefers_current_system_metadata_and_titles() {
        let mut street = game("Street Fighter II", "/games/sf2.mra", "", "arcade");
        street.manufacturer = "Capcom".into();
        street.category = "Fighter / 2D".into();
        let mut pac = game("Pac-Man", "/games/pacman.mra", "", "arcade");
        pac.manufacturer = "Namco".into();
        pac.category = "Maze / Pac-Man".into();
        let mut shooter = game("1942", "/games/1942.mra", "", "arcade");
        shooter.year = Some(1984);
        shooter.manufacturer = "Capcom".into();
        shooter.category = "Shooter / Vertical".into();
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
        assert_eq!(catalog.autocomplete_search_word("arcade", "sho"), "shooter");
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

        let catalog = ArcadeCatalog::new(PathBuf::from("/media/fat/_Arcade"), catalog_games, Vec::new());

        assert_eq!(catalog.autocomplete_search_word("arcade", "cap"), "capcom");
    }

    #[test]
    fn arcade_search_real_fixture_expectations_when_available() {
        let Some(catalog) = optional_real_fixture_catalog() else {
            return;
        };

        assert!(catalog.search_game_indexes("arcade", "capcom").len() >= 50);
        assert!(catalog.search_game_indexes("arcade", "maze").len() >= 10);
        assert!(!catalog.search_game_indexes("arcade", "street fighter").is_empty());
        assert!(!catalog.search_game_indexes("arcade", "pac man").is_empty());
        assert!(!catalog.search_game_indexes("arcade", "194").is_empty());

        assert_eq!(catalog.autocomplete_search_word("arcade", "cap"), "capcom");
        assert_eq!(catalog.autocomplete_search_word("arcade", "seg"), "sega");
        assert_eq!(catalog.autocomplete_search_word("arcade", "kon"), "konami");
        assert_eq!(catalog.autocomplete_search_word("arcade", "str"), "street");
        assert_eq!(catalog.autocomplete_search_word("arcade", "fig"), "fighter");
        assert_eq!(catalog.autocomplete_search_word("arcade", "pac"), "pac");
        assert_eq!(catalog.autocomplete_search_word("arcade", "maz"), "maze");
        assert_eq!(catalog.autocomplete_search_word("arcade", "sho"), "shooter");
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
            games.push(ArcadeGameEntry {
                system_id: columns[0].into(),
                title: columns[1].into(),
                mra_path: columns[2].into(),
                year,
                manufacturer: columns[4].into(),
                category: columns[5].into(),
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
    fn systems_from_games_uses_runtime_order_and_human_titles() {
        let games = vec![
            ArcadeGameEntry {
                title: "Unknown Thing".into(),
                mra_path: "/media/fat/_Arcade/Unknown.mra".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "unknown".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                is_new: false,
            },
            ArcadeGameEntry {
                title: "Sonic".into(),
                mra_path: "/media/fat/games/MegaDrive/Sonic.md".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "megadrive".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
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
                is_new: false,
            },
            ArcadeGameEntry {
                title: "Another Sonic".into(),
                mra_path: "/media/fat/games/MegaDrive/Another Sonic.md".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "megadrive".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                is_new: false,
            },
        ];

        let systems = systems_from_games(&games);

        assert_eq!(
            systems,
            vec![
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
                    id: "unknown".into(),
                    title: "Unknown".into(),
                    count: 1
                }
            ]
        );
    }
}
