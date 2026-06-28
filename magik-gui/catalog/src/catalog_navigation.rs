//! Runtime navigation projection for fast launcher catalog hydration.

use crate::arcade_catalog::{
    ArcadeCatalog, ArcadeGameEntry, GameSystemEntry, LaunchTarget, StructuredLaunchPlan,
};
use crate::catalog_config::{CATALOG_BUILD_VERSION, SCHEMA_VERSION};
use crate::catalog_load_metrics;
use crate::catalog_stamp::CatalogStamp;
use crate::preview_worker;
use crate::sqlite_catalog;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const CATALOG_NAVIGATION_SCHEMA_VERSION: u32 = 5;
const CATALOG_NAVIGATION_BINARY_MAGIC: &[u8; 8] = b"MMNAVB5\0";
const NAV_REF_FULL: u8 = 0;
const NAV_REF_PAYLOAD: u8 = 1;
const NAV_REF_ARCHIVE: u8 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogNavigationProjection {
    pub schema: u32,
    pub catalog_schema_version: u32,
    pub catalog_build_version: u32,
    pub catalog_generation: String,
    pub catalog_stamp_fingerprint: String,
    pub catalog_stamp_lines: Vec<String>,
    pub systems: Vec<NavigationSystem>,
    pub games: Vec<NavigationGame>,
    pub launch_plans: Vec<NavigationLaunchPlan>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationSystem {
    pub id: String,
    pub title: String,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationGame {
    pub title: Arc<str>,
    pub launch_ref: Arc<str>,
    pub preview_archive_path: Arc<str>,
    pub preview_asset_key: Arc<str>,
    pub has_preview: bool,
    pub system_id: Arc<str>,
    pub year: Option<u16>,
    pub manufacturer: Arc<str>,
    pub category: Arc<str>,
    pub is_new: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationLaunchPlan {
    pub launch_ref: Arc<str>,
    pub title: Arc<str>,
    pub system_id: Arc<str>,
    pub core_path: Arc<str>,
    pub payload_path: Arc<str>,
    pub mount_kind: Arc<str>,
    pub mount_index: u8,
    pub delay_secs: u8,
}

pub fn navigation_path_for_sqlite(sqlite_path: &Path) -> PathBuf {
    sqlite_path.with_extension("nav.lz4b")
}

pub fn write_catalog_navigation_projection_for_catalog(
    sqlite_path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &CatalogStamp,
) -> Result<(), String> {
    let projection = CatalogNavigationProjection::from_catalog(catalog, stamp);
    write_catalog_navigation_projection_for_sqlite(sqlite_path, &projection)
}

pub(crate) fn write_catalog_navigation_projection_for_sqlite(
    sqlite_path: &Path,
    projection: &CatalogNavigationProjection,
) -> Result<(), String> {
    write_catalog_navigation_projection(&navigation_path_for_sqlite(sqlite_path), projection)
}

pub fn read_catalog_navigation_projection(
    path: &Path,
    expected_stamp: &CatalogStamp,
) -> Result<Option<CatalogNavigationProjection>, String> {
    catalog_load_metrics::record_nav_projection_read();
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read catalog navigation {}: {e}", path.display())),
    };
    let decoded = lz4_flex::decompress_size_prepended(&bytes)
        .map_err(|e| format!("decompress catalog navigation {}: {e}", path.display()))?;
    let projection = decode_navigation_projection(&decoded)
        .map_err(|e| format!("parse catalog navigation {}: {e}", path.display()))?;
    if !projection.matches(expected_stamp) {
        return Ok(None);
    }
    Ok(Some(projection))
}

impl CatalogNavigationProjection {
    pub fn from_catalog(catalog: &ArcadeCatalog, stamp: &CatalogStamp) -> Self {
        let catalog_stamp_fingerprint = stamp.fingerprint_hex();
        Self {
            schema: CATALOG_NAVIGATION_SCHEMA_VERSION,
            catalog_schema_version: SCHEMA_VERSION,
            catalog_build_version: CATALOG_BUILD_VERSION,
            catalog_generation: catalog_stamp_fingerprint.clone(),
            catalog_stamp_fingerprint,
            catalog_stamp_lines: stamp.lines().to_vec(),
            systems: catalog.systems.iter().map(NavigationSystem::from).collect(),
            games: catalog.games.iter().map(NavigationGame::from).collect(),
            launch_plans: structured_launch_plans(catalog),
        }
    }

    pub fn matches(&self, expected_stamp: &CatalogStamp) -> bool {
        self.schema == CATALOG_NAVIGATION_SCHEMA_VERSION
            && self.catalog_schema_version == SCHEMA_VERSION
            && self.catalog_build_version == CATALOG_BUILD_VERSION
            && self.catalog_stamp_fingerprint == expected_stamp.fingerprint_hex()
            && self.catalog_stamp_lines == expected_stamp.lines()
    }
}

impl From<&GameSystemEntry> for NavigationSystem {
    fn from(system: &GameSystemEntry) -> Self {
        Self {
            id: system.id.clone(),
            title: system.title.clone(),
            count: system.count,
        }
    }
}

impl From<NavigationSystem> for GameSystemEntry {
    fn from(system: NavigationSystem) -> Self {
        Self {
            id: system.id,
            title: system.title,
            count: system.count,
        }
    }
}

impl From<&ArcadeGameEntry> for NavigationGame {
    fn from(game: &ArcadeGameEntry) -> Self {
        Self {
            title: game.title.clone(),
            launch_ref: game.mra_path.clone(),
            preview_archive_path: game.preview_archive_path.clone(),
            preview_asset_key: game.preview_asset_key.clone(),
            has_preview: game.has_preview,
            system_id: game.system_id.clone(),
            year: game.year,
            manufacturer: game.manufacturer.clone(),
            category: game.category.clone(),
            is_new: game.is_new,
        }
    }
}

impl From<NavigationGame> for ArcadeGameEntry {
    fn from(game: NavigationGame) -> Self {
        Self {
            title: game.title,
            mra_path: game.launch_ref,
            preview_archive_path: game.preview_archive_path,
            preview_asset_key: game.preview_asset_key,
            has_preview: game.has_preview,
            system_id: game.system_id,
            year: game.year,
            manufacturer: game.manufacturer,
            category: game.category,
            is_new: game.is_new,
        }
    }
}

impl From<&StructuredLaunchPlan> for NavigationLaunchPlan {
    fn from(plan: &StructuredLaunchPlan) -> Self {
        Self {
            launch_ref: plan.launch_ref.clone(),
            title: plan.title.clone(),
            system_id: plan.system_id.clone(),
            core_path: plan.core_path.clone(),
            payload_path: plan.payload_path.clone(),
            mount_kind: plan.mount_kind.clone(),
            mount_index: plan.mount_index,
            delay_secs: plan.delay_secs,
        }
    }
}

impl From<NavigationLaunchPlan> for StructuredLaunchPlan {
    fn from(plan: NavigationLaunchPlan) -> Self {
        Self {
            launch_ref: plan.launch_ref,
            title: plan.title,
            system_id: plan.system_id,
            core_path: plan.core_path,
            payload_path: plan.payload_path,
            mount_kind: plan.mount_kind,
            mount_index: plan.mount_index,
            delay_secs: plan.delay_secs,
        }
    }
}

fn structured_launch_plans(catalog: &ArcadeCatalog) -> Vec<NavigationLaunchPlan> {
    let mut seen = HashSet::new();
    let mut plans = Vec::new();
    for game in &catalog.games {
        if !seen.insert(game.mra_path.to_string()) {
            continue;
        }
        if let LaunchTarget::Structured(plan) =
            catalog.launch_target_for_ref(game.mra_path.as_ref())
        {
            plans.push(NavigationLaunchPlan::from(&plan));
        }
    }
    plans
}

fn write_catalog_navigation_projection(
    navigation_path: &Path,
    projection: &CatalogNavigationProjection,
) -> Result<(), String> {
    let encoded = encode_navigation_projection(projection)?;
    let compressed = lz4_flex::compress_prepend_size(&encoded);
    write_bytes_atomically(navigation_path, &compressed)
}

struct CompactNavigationProjection<'a> {
    launch_defaults: Vec<NavigationLaunchDefault>,
    games: Vec<CompactNavigationGame<'a>>,
    launch_plans: Vec<CompactNavigationLaunchPlan<'a>>,
}

struct CompactNavigationGame<'a> {
    title: &'a str,
    launch_ref: CompactGameLaunchRef<'a>,
    preview_asset_key: &'a str,
    has_preview: bool,
    system_id: &'a str,
    year: Option<u16>,
    manufacturer: &'a str,
    category: &'a str,
    is_new: bool,
}

enum CompactGameLaunchRef<'a> {
    Full(&'a str),
    PlanIndex(u32),
}

struct CompactNavigationLaunchPlan<'a> {
    game_index: u32,
    ref_kind: u8,
    full_launch_ref: Option<&'a str>,
    payload_path: &'a str,
    core_path_override: Option<&'a str>,
    mount_override: Option<NavigationLaunchMount>,
}

#[derive(Clone)]
struct NavigationLaunchDefault {
    system_id: String,
    core_path: Arc<str>,
    mount_kind: Arc<str>,
    mount_index: u8,
    delay_secs: u8,
}

#[derive(Clone)]
struct NavigationLaunchMount {
    mount_kind: Arc<str>,
    mount_index: u8,
    delay_secs: u8,
}

struct CompactDecodedGame {
    title: Arc<str>,
    launch_ref: CompactDecodedGameLaunchRef,
    preview_archive_path: Arc<str>,
    preview_asset_key: Arc<str>,
    has_preview: bool,
    system_id: Arc<str>,
    year: Option<u16>,
    manufacturer: Arc<str>,
    category: Arc<str>,
    is_new: bool,
}

enum CompactDecodedGameLaunchRef {
    Full(Arc<str>),
    PlanIndex(usize),
}

impl<'a> CompactNavigationProjection<'a> {
    fn from_projection(projection: &'a CatalogNavigationProjection) -> Result<Self, String> {
        let plan_by_ref = projection
            .launch_plans
            .iter()
            .enumerate()
            .map(|(idx, plan)| (plan.launch_ref.as_ref(), (idx, plan)))
            .collect::<HashMap<_, _>>();
        let game_index_by_ref = projection
            .games
            .iter()
            .enumerate()
            .map(|(idx, game)| (game.launch_ref.as_ref(), idx))
            .collect::<HashMap<_, _>>();

        let mut default_candidates = BTreeMap::<&str, BTreeMap<(&str, &str, u8, u8), usize>>::new();
        for plan in &projection.launch_plans {
            *default_candidates
                .entry(plan.system_id.as_ref())
                .or_default()
                .entry((
                    plan.core_path.as_ref(),
                    plan.mount_kind.as_ref(),
                    plan.mount_index,
                    plan.delay_secs,
                ))
                .or_default() += 1;
        }
        let mut defaults_by_system = HashMap::<&str, NavigationLaunchDefault>::new();
        let mut launch_defaults = Vec::new();
        for (system_id, candidates) in default_candidates {
            let ((core_path, mount_kind, mount_index, delay_secs), _) = candidates
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .ok_or_else(|| format!("navigation launch defaults missing for {system_id}"))?;
            let default = NavigationLaunchDefault {
                system_id: system_id.to_string(),
                core_path: Arc::from(core_path),
                mount_kind: Arc::from(mount_kind),
                mount_index,
                delay_secs,
            };
            defaults_by_system.insert(system_id, default.clone());
            launch_defaults.push(default);
        }

        let mut games = Vec::with_capacity(projection.games.len());
        for game in &projection.games {
            let launch_ref =
                if let Some((plan_index, plan)) = plan_by_ref.get(game.launch_ref.as_ref()) {
                    if compact_launch_ref_kind(plan.launch_ref.as_ref())
                        .and_then(|kind| launch_ref_from_kind(kind, &plan.payload_path).ok())
                        .is_some_and(|expected| expected.as_ref() == game.launch_ref.as_ref())
                    {
                        CompactGameLaunchRef::PlanIndex(
                            (*plan_index)
                                .try_into()
                                .map_err(|_| "navigation plan index too large".to_string())?,
                        )
                    } else {
                        CompactGameLaunchRef::Full(game.launch_ref.as_ref())
                    }
                } else {
                    CompactGameLaunchRef::Full(game.launch_ref.as_ref())
                };
            games.push(CompactNavigationGame {
                title: game.title.as_ref(),
                launch_ref,
                preview_asset_key: game.preview_asset_key.as_ref(),
                has_preview: game.has_preview,
                system_id: game.system_id.as_ref(),
                year: game.year,
                manufacturer: game.manufacturer.as_ref(),
                category: game.category.as_ref(),
                is_new: game.is_new,
            });
        }

        let mut launch_plans = Vec::with_capacity(projection.launch_plans.len());
        for plan in &projection.launch_plans {
            let game_index = game_index_by_ref
                .get(plan.launch_ref.as_ref())
                .ok_or_else(|| {
                    format!("navigation launch plan has no game: {}", plan.launch_ref)
                })?;
            let default = defaults_by_system
                .get(plan.system_id.as_ref())
                .ok_or_else(|| {
                    format!("navigation launch default missing for {}", plan.system_id)
                })?;
            let core_path_override = if plan.core_path == default.core_path {
                None
            } else {
                Some(plan.core_path.as_ref())
            };
            let mount_override = if plan.mount_kind == default.mount_kind
                && plan.mount_index == default.mount_index
                && plan.delay_secs == default.delay_secs
            {
                None
            } else {
                Some(NavigationLaunchMount {
                    mount_kind: plan.mount_kind.clone(),
                    mount_index: plan.mount_index,
                    delay_secs: plan.delay_secs,
                })
            };
            launch_plans.push(CompactNavigationLaunchPlan {
                game_index: (*game_index)
                    .try_into()
                    .map_err(|_| "navigation game index too large".to_string())?,
                ref_kind: compact_launch_ref_kind(plan.launch_ref.as_ref()).unwrap_or(NAV_REF_FULL),
                full_launch_ref: compact_launch_ref_kind(plan.launch_ref.as_ref())
                    .is_none()
                    .then_some(plan.launch_ref.as_ref()),
                payload_path: plan.payload_path.as_ref(),
                core_path_override,
                mount_override,
            });
        }

        Ok(Self {
            launch_defaults,
            games,
            launch_plans,
        })
    }
}

fn compact_launch_ref_kind(launch_ref: &str) -> Option<u8> {
    if launch_ref.starts_with("magik-plan:payload:") {
        Some(NAV_REF_PAYLOAD)
    } else if launch_ref.starts_with("magik-plan:archive:") {
        Some(NAV_REF_ARCHIVE)
    } else {
        None
    }
}

fn launch_ref_from_kind(ref_kind: u8, payload_path: &str) -> Result<Arc<str>, String> {
    match ref_kind {
        NAV_REF_PAYLOAD => Ok(Arc::from(format!("magik-plan:payload:{payload_path}"))),
        NAV_REF_ARCHIVE => Ok(Arc::from(format!("magik-plan:archive:{payload_path}"))),
        value => Err(format!(
            "navigation compact launch ref kind {value} is invalid"
        )),
    }
}

fn encode_navigation_projection(
    projection: &CatalogNavigationProjection,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let compact = CompactNavigationProjection::from_projection(projection)?;
    out.extend_from_slice(CATALOG_NAVIGATION_BINARY_MAGIC);
    write_u32(&mut out, projection.schema);
    write_u32(&mut out, projection.catalog_schema_version);
    write_u32(&mut out, projection.catalog_build_version);
    write_string(&mut out, &projection.catalog_generation)?;
    write_string(&mut out, &projection.catalog_stamp_fingerprint)?;
    write_len(&mut out, projection.catalog_stamp_lines.len())?;
    for line in &projection.catalog_stamp_lines {
        write_string(&mut out, line)?;
    }
    write_len(&mut out, projection.systems.len())?;
    for system in &projection.systems {
        write_string(&mut out, &system.id)?;
        write_string(&mut out, &system.title)?;
        write_u64(&mut out, system.count as u64);
    }
    write_len(&mut out, compact.launch_defaults.len())?;
    for default in &compact.launch_defaults {
        write_string(&mut out, &default.system_id)?;
        write_string(&mut out, &default.core_path)?;
        write_string(&mut out, &default.mount_kind)?;
        out.push(default.mount_index);
        out.push(default.delay_secs);
    }
    write_len(&mut out, compact.games.len())?;
    for game in &compact.games {
        write_string(&mut out, game.title)?;
        match &game.launch_ref {
            CompactGameLaunchRef::Full(launch_ref) => {
                out.push(NAV_REF_FULL);
                write_string(&mut out, launch_ref)?;
            }
            CompactGameLaunchRef::PlanIndex(plan_index) => {
                out.push(NAV_REF_PAYLOAD);
                write_u32(&mut out, *plan_index);
            }
        }
        write_string(&mut out, game.preview_asset_key)?;
        write_bool(&mut out, game.has_preview);
        write_string(&mut out, game.system_id)?;
        match game.year {
            Some(year) => {
                write_bool(&mut out, true);
                write_u16(&mut out, year);
            }
            None => write_bool(&mut out, false),
        }
        write_string(&mut out, game.manufacturer)?;
        write_string(&mut out, game.category)?;
        write_bool(&mut out, game.is_new);
    }
    write_len(&mut out, compact.launch_plans.len())?;
    for plan in &compact.launch_plans {
        write_u32(&mut out, plan.game_index);
        out.push(plan.ref_kind);
        if plan.ref_kind == NAV_REF_FULL {
            write_string(
                &mut out,
                plan.full_launch_ref
                    .ok_or_else(|| "navigation full launch ref missing".to_string())?,
            )?;
        }
        write_string(&mut out, plan.payload_path)?;
        match &plan.core_path_override {
            Some(value) => {
                write_bool(&mut out, true);
                write_string(&mut out, value)?;
            }
            None => write_bool(&mut out, false),
        }
        match &plan.mount_override {
            Some(value) => {
                write_bool(&mut out, true);
                write_string(&mut out, &value.mount_kind)?;
                out.push(value.mount_index);
                out.push(value.delay_secs);
            }
            None => write_bool(&mut out, false),
        }
    }
    Ok(out)
}

fn decode_navigation_projection(bytes: &[u8]) -> Result<CatalogNavigationProjection, String> {
    let mut reader = NavigationBinaryReader::new(bytes);
    reader.expect_magic(CATALOG_NAVIGATION_BINARY_MAGIC)?;
    let schema = reader.read_u32()?;
    let catalog_schema_version = reader.read_u32()?;
    let catalog_build_version = reader.read_u32()?;
    let catalog_generation = reader.read_string()?;
    let catalog_stamp_fingerprint = reader.read_string()?;
    let stamp_line_count = reader.read_len()?;
    let mut catalog_stamp_lines = Vec::with_capacity(stamp_line_count);
    for _ in 0..stamp_line_count {
        catalog_stamp_lines.push(reader.read_string()?);
    }
    let system_count = reader.read_len()?;
    let mut systems = Vec::with_capacity(system_count);
    for _ in 0..system_count {
        systems.push(NavigationSystem {
            id: reader.read_string()?,
            title: reader.read_string()?,
            count: reader
                .read_u64()?
                .try_into()
                .map_err(|_| "system count too large".to_string())?,
        });
    }
    let launch_default_count = reader.read_len()?;
    let mut launch_defaults = HashMap::<String, NavigationLaunchDefault>::new();
    for _ in 0..launch_default_count {
        let system_id = reader.read_string()?;
        let default = NavigationLaunchDefault {
            system_id: system_id.clone(),
            core_path: Arc::from(reader.read_string()?),
            mount_kind: Arc::from(reader.read_string()?),
            mount_index: reader.read_u8()?,
            delay_secs: reader.read_u8()?,
        };
        launch_defaults.insert(system_id, default);
    }
    let game_count = reader.read_len()?;
    let mut game_rows = Vec::with_capacity(game_count);
    let mut preview_archive_paths_by_system = HashMap::<String, Arc<str>>::new();
    for _ in 0..game_count {
        let title = reader.read_arc_string()?;
        let launch_ref = match reader.read_u8()? {
            NAV_REF_FULL => CompactDecodedGameLaunchRef::Full(reader.read_arc_string()?),
            NAV_REF_PAYLOAD => CompactDecodedGameLaunchRef::PlanIndex(reader.read_u32()? as usize),
            value => {
                return Err(format!(
                    "navigation projection launch ref mode {value} is invalid"
                ))
            }
        };
        let preview_asset_key = reader.read_arc_string()?;
        let has_preview = reader.read_bool()?;
        let system_id = reader.read_arc_string()?;
        let preview_archive_path = if preview_asset_key.is_empty() {
            Arc::from("")
        } else {
            preview_archive_paths_by_system
                .entry(system_id.to_string())
                .or_insert_with(|| {
                    preview_worker::preview_archive_path_for_system(&system_id).into()
                })
                .clone()
        };
        let year = if reader.read_bool()? {
            Some(reader.read_u16()?)
        } else {
            None
        };
        let manufacturer = reader.read_arc_string()?;
        let category = reader.read_arc_string()?;
        let is_new = reader.read_bool()?;
        game_rows.push(CompactDecodedGame {
            title,
            launch_ref,
            preview_archive_path,
            preview_asset_key,
            has_preview,
            system_id,
            year,
            manufacturer,
            category,
            is_new,
        });
    }
    let launch_plan_count = reader.read_len()?;
    let mut launch_plans = Vec::with_capacity(launch_plan_count);
    for _ in 0..launch_plan_count {
        let game_index = reader.read_u32()? as usize;
        let ref_kind = reader.read_u8()?;
        let full_launch_ref = if ref_kind == NAV_REF_FULL {
            Some(reader.read_arc_string()?)
        } else {
            None
        };
        let payload_path = reader.read_arc_string()?;
        let core_path_override = if reader.read_bool()? {
            Some(reader.read_arc_string()?)
        } else {
            None
        };
        let mount_override = if reader.read_bool()? {
            Some(NavigationLaunchMount {
                mount_kind: reader.read_arc_string()?,
                mount_index: reader.read_u8()?,
                delay_secs: reader.read_u8()?,
            })
        } else {
            None
        };
        let game = game_rows
            .get(game_index)
            .ok_or_else(|| format!("navigation launch plan game index {game_index} is invalid"))?;
        let launch_ref = if let Some(launch_ref) = full_launch_ref {
            launch_ref
        } else {
            launch_ref_from_kind(ref_kind, &payload_path)?
        };
        let default = launch_defaults
            .get(game.system_id.as_ref())
            .ok_or_else(|| format!("navigation launch defaults missing for {}", game.system_id))?;
        let mount = mount_override.unwrap_or_else(|| NavigationLaunchMount {
            mount_kind: default.mount_kind.clone(),
            mount_index: default.mount_index,
            delay_secs: default.delay_secs,
        });
        launch_plans.push(NavigationLaunchPlan {
            launch_ref,
            title: game.title.clone(),
            system_id: game.system_id.clone(),
            core_path: core_path_override.unwrap_or_else(|| default.core_path.clone()),
            payload_path,
            mount_kind: mount.mount_kind,
            mount_index: mount.mount_index,
            delay_secs: mount.delay_secs,
        });
    }
    let mut games = Vec::with_capacity(game_rows.len());
    let launch_refs_by_plan = launch_plans
        .iter()
        .map(|plan| plan.launch_ref.clone())
        .collect::<Vec<_>>();
    for game in game_rows {
        let launch_ref = match game.launch_ref {
            CompactDecodedGameLaunchRef::Full(launch_ref) => launch_ref,
            CompactDecodedGameLaunchRef::PlanIndex(plan_index) => launch_refs_by_plan
                .get(plan_index)
                .cloned()
                .ok_or_else(|| format!("navigation game plan index {plan_index} is invalid"))?,
        };
        games.push(NavigationGame {
            title: game.title,
            launch_ref,
            preview_archive_path: game.preview_archive_path,
            preview_asset_key: game.preview_asset_key,
            has_preview: game.has_preview,
            system_id: game.system_id,
            year: game.year,
            manufacturer: game.manufacturer,
            category: game.category,
            is_new: game.is_new,
        });
    }
    reader.finish()?;
    Ok(CatalogNavigationProjection {
        schema,
        catalog_schema_version,
        catalog_build_version,
        catalog_generation,
        catalog_stamp_fingerprint,
        catalog_stamp_lines,
        systems,
        games,
        launch_plans,
    })
}

fn write_len(out: &mut Vec<u8>, value: usize) -> Result<(), String> {
    let value: u32 = value
        .try_into()
        .map_err(|_| "catalog navigation collection too large".to_string())?;
    write_u32(out, value);
    Ok(())
}

fn write_string(out: &mut Vec<u8>, value: &str) -> Result<(), String> {
    write_len(out, value.len())?;
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn write_bool(out: &mut Vec<u8>, value: bool) {
    out.push(u8::from(value));
}

fn write_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

struct NavigationBinaryReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> NavigationBinaryReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn expect_magic(&mut self, magic: &[u8]) -> Result<(), String> {
        let bytes = self.take(magic.len())?;
        if bytes == magic {
            Ok(())
        } else {
            Err("navigation projection magic mismatch".to_string())
        }
    }

    fn read_len(&mut self) -> Result<usize, String> {
        self.read_u32()?
            .try_into()
            .map_err(|_| "navigation projection length too large".to_string())
    }

    fn read_string(&mut self) -> Result<String, String> {
        let len = self.read_len()?;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|e| format!("navigation projection string is not utf-8: {e}"))
    }

    fn read_arc_string(&mut self) -> Result<Arc<str>, String> {
        let len = self.read_len()?;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(Arc::from)
            .map_err(|e| format!("navigation projection string is not utf-8: {e}"))
    }

    fn read_bool(&mut self) -> Result<bool, String> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(format!(
                "navigation projection bool value {value} is invalid"
            )),
        }
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| "navigation projection offset overflow".to_string())?;
        if end > self.bytes.len() {
            return Err("navigation projection is truncated".to_string());
        }
        let bytes = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn finish(self) -> Result<(), String> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err("navigation projection has trailing bytes".to_string())
        }
    }
}

fn write_bytes_atomically(final_path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create catalog navigation dir {}: {e}", parent.display()))?;
    }
    let temp_path = navigation_temp_path_for(final_path);
    let result = (|| -> Result<(), String> {
        let mut file = File::create(&temp_path).map_err(|e| {
            format!(
                "create catalog navigation temp {}: {e}",
                temp_path.display()
            )
        })?;
        file.write_all(bytes)
            .map_err(|e| format!("write catalog navigation temp {}: {e}", temp_path.display()))?;
        file.sync_all()
            .map_err(|e| format!("sync catalog navigation temp {}: {e}", temp_path.display()))?;
        drop(file);
        std::fs::rename(&temp_path, final_path).map_err(|e| {
            format!(
                "replace catalog navigation {} from {}: {e}",
                final_path.display(),
                temp_path.display()
            )
        })?;
        sqlite_catalog::sync_parent_dir(final_path);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn navigation_temp_path_for(final_path: &Path) -> PathBuf {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("catalog.nav.lz4b");
    final_path.with_file_name(format!(".{file_name}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn stamp(lines: &[&str]) -> CatalogStamp {
        CatalogStamp::from_lines(lines.iter().map(|line| line.to_string()).collect())
    }

    fn game(title: &str, launch_ref: &str, system_id: &str) -> ArcadeGameEntry {
        ArcadeGameEntry {
            title: Arc::from(title),
            mra_path: Arc::from(launch_ref),
            preview_archive_path: Arc::from("/media/fat/mister-magik/assets/arcade.mmlz4b"),
            preview_asset_key: Arc::from(title.to_ascii_lowercase()),
            has_preview: true,
            system_id: Arc::from(system_id),
            year: Some(1984),
            manufacturer: Arc::from("Capcom"),
            category: Arc::from("Shooter"),
            is_new: false,
        }
    }

    fn projection_catalog() -> ArcadeCatalog {
        let saturn_payload = "/media/fat/games/Saturn/Nights.chd";
        let neogeo_payload = "/media/fat/games/NEOGEO/Pack.zip/Pack/World A-Z/mslug.neo";
        let games = vec![
            game("1942", "/media/fat/_Arcade/1942.mra", "arcade"),
            game(
                "Nights",
                &format!("magik-plan:payload:{saturn_payload}"),
                "saturn",
            ),
            game(
                "Metal Slug",
                &format!("magik-plan:archive:{neogeo_payload}"),
                "neogeo",
            ),
        ];
        let systems = vec![
            GameSystemEntry {
                id: "arcade".to_string(),
                title: "Arcade".to_string(),
                count: 1,
            },
            GameSystemEntry {
                id: "saturn".to_string(),
                title: "Saturn".to_string(),
                count: 1,
            },
            GameSystemEntry {
                id: "neogeo".to_string(),
                title: "NeoGeo".to_string(),
                count: 1,
            },
        ];
        let plans = vec![
            StructuredLaunchPlan {
                launch_ref: Arc::from(format!("magik-plan:payload:{saturn_payload}")),
                title: Arc::from("Nights"),
                system_id: Arc::from("saturn"),
                core_path: Arc::from("_Console/Saturn"),
                payload_path: Arc::from(saturn_payload),
                mount_kind: Arc::from("mount-image"),
                mount_index: 0,
                delay_secs: 1,
            },
            StructuredLaunchPlan {
                launch_ref: Arc::from(format!("magik-plan:archive:{neogeo_payload}")),
                title: Arc::from("Metal Slug"),
                system_id: Arc::from("neogeo"),
                core_path: Arc::from("_Console/NeoGeo"),
                payload_path: Arc::from(neogeo_payload),
                mount_kind: Arc::from("load-file"),
                mount_index: 1,
                delay_secs: 1,
            },
        ];
        ArcadeCatalog::new_with_launch_plans(
            PathBuf::from("/media/fat/_Arcade"),
            games,
            systems,
            plans,
        )
    }

    #[test]
    fn navigation_projection_round_trips_catalog_rows_and_plans() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-navigation-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp dir");
        let db = root.join("library.sqlite3");
        let stamp = stamp(&["root /media/fat/games"]);
        let catalog = projection_catalog();

        write_catalog_navigation_projection_for_catalog(&db, &catalog, &stamp)
            .expect("write projection");
        let loaded = read_catalog_navigation_projection(&navigation_path_for_sqlite(&db), &stamp)
            .expect("read projection")
            .expect("current projection");
        let hydrated =
            ArcadeCatalog::from_navigation_projection("/media/fat/_Arcade", loaded.clone());

        assert_eq!(loaded.games.len(), catalog.games.len());
        assert_eq!(loaded.systems.len(), catalog.systems.len());
        assert_eq!(loaded.launch_plans.len(), 2);
        assert_eq!(hydrated.games.len(), catalog.games.len());
        assert_eq!(hydrated.systems, catalog.systems);
        assert_eq!(hydrated.decade_option_count("arcade"), 1);
        assert_eq!(hydrated.manufacturer_option_count("arcade"), 1);
        assert_eq!(hydrated.category_option_count("arcade"), 1);
        assert!(matches!(
            hydrated.launch_target_for_ref("magik-plan:payload:/media/fat/games/Saturn/Nights.chd"),
            LaunchTarget::Structured(_)
        ));
        assert!(matches!(
            hydrated.launch_target_for_ref(
                "magik-plan:archive:/media/fat/games/NEOGEO/Pack.zip/Pack/World A-Z/mslug.neo"
            ),
            LaunchTarget::Structured(_)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn navigation_projection_rejects_stale_stamp_and_corrupt_file() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-navigation-stale-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp dir");
        let db = root.join("library.sqlite3");
        let path = navigation_path_for_sqlite(&db);
        let current_stamp = stamp(&["root current"]);
        let stale_stamp = stamp(&["root stale"]);

        write_catalog_navigation_projection_for_catalog(&db, &projection_catalog(), &current_stamp)
            .expect("write projection");
        assert!(read_catalog_navigation_projection(&path, &stale_stamp)
            .expect("read stale projection")
            .is_none());
        std::fs::write(&path, b"not-lz4").expect("write corrupt projection");
        assert!(read_catalog_navigation_projection(&path, &current_stamp).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
