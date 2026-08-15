// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runtime navigation projection for fast launcher catalog hydration.

#[cfg(test)]
use crate::arcade_catalog::LaunchTarget;
use crate::arcade_catalog::{
    ArcadeCatalog, ArcadeGameEntry, GameSystemEntry, PlatformKind, StructuredLaunchPlan,
};
use crate::bounded_lz4;
use crate::catalog_config::{CATALOG_BUILD_VERSION, SCHEMA_VERSION};
use crate::catalog_load_metrics;
use crate::catalog_stamp::CatalogStamp;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const CATALOG_NAVIGATION_SCHEMA_VERSION: u32 = 10;
const CATALOG_NAVIGATION_BINARY_MAGIC: &[u8; 8] = b"MMNAV10\0";
const NAV_REF_FULL: u8 = 0;
const NAV_REF_PAYLOAD: u8 = 1;
const NAV_REF_ARCHIVE: u8 = 2;
const MAX_CATALOG_NAVIGATION_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_CATALOG_NAVIGATION_COMPRESSED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CATALOG_NAVIGATION_ITEMS: usize = 100_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NavigationSnapshotWriteTiming {
    pub(crate) conversion_us: u64,
    pub(crate) encode_us: u64,
    pub(crate) compress_us: u64,
    pub(crate) write_us: u64,
    pub(crate) total_us: u64,
    pub(crate) encoded_bytes: usize,
    pub(crate) compressed_bytes: usize,
}

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

pub struct CatalogNavigationProjectionRead {
    pub projection: CatalogNavigationProjection,
    pub file_read_us: u64,
    pub decompress_us: u64,
    pub decode_us: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationSystem {
    pub id: String,
    pub title: String,
    pub count: usize,
    pub platform_kind: PlatformKind,
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
    pub players: Option<u8>,
    pub control: Arc<str>,
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

pub fn write_catalog_navigation_snapshot(
    path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &CatalogStamp,
) -> Result<(), String> {
    write_catalog_navigation_snapshot_with_timing(path, catalog, stamp).map(|_| ())
}

pub(crate) fn write_catalog_navigation_snapshot_with_timing(
    path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &CatalogStamp,
) -> Result<NavigationSnapshotWriteTiming, String> {
    let mut fault_control = crate::fs_fault::NoopDirectResetFaultControl;
    write_catalog_navigation_snapshot_with_timing_and_fault_control(
        path,
        catalog,
        stamp,
        &mut fault_control,
    )
}

pub(crate) fn write_catalog_navigation_snapshot_with_timing_and_fault_control(
    path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &CatalogStamp,
    fault_control: &mut dyn crate::fs_fault::DirectResetFaultControl,
) -> Result<NavigationSnapshotWriteTiming, String> {
    let total_started = std::time::Instant::now();
    let conversion_started = std::time::Instant::now();
    let projection = CatalogNavigationProjection::from_catalog(catalog, stamp);
    let conversion_us = conversion_started.elapsed().as_micros() as u64;

    let encode_started = std::time::Instant::now();
    let encoded = encode_navigation_projection(&projection)?;
    let encode_us = encode_started.elapsed().as_micros() as u64;
    let encoded_bytes = encoded.len();

    let compress_started = std::time::Instant::now();
    let compressed = lz4_flex::compress_prepend_size(&encoded);
    let compress_us = compress_started.elapsed().as_micros() as u64;
    let compressed_bytes = compressed.len();

    let write_started = std::time::Instant::now();
    write_bytes_atomically_with_fault_control(path, &compressed, fault_control)?;
    let write_us = write_started.elapsed().as_micros() as u64;

    Ok(NavigationSnapshotWriteTiming {
        conversion_us,
        encode_us,
        compress_us,
        write_us,
        total_us: total_started.elapsed().as_micros() as u64,
        encoded_bytes,
        compressed_bytes,
    })
}

pub(crate) fn encode_catalog_navigation_for_storage(
    catalog: &ArcadeCatalog,
    stamp: &CatalogStamp,
) -> Result<Vec<u8>, String> {
    let projection = CatalogNavigationProjection::from_catalog(catalog, stamp);
    let encoded = encode_navigation_projection(&projection)?;
    Ok(lz4_flex::compress_prepend_size(&encoded))
}

pub(crate) fn decode_catalog_navigation_from_storage(
    compressed: &[u8],
    expected_stamp: &CatalogStamp,
) -> Result<Option<CatalogNavigationProjection>, String> {
    if compressed.len() as u64 > MAX_CATALOG_NAVIGATION_COMPRESSED_BYTES {
        return Err(format!(
            "embedded catalog navigation compressed size {} exceeds max {MAX_CATALOG_NAVIGATION_COMPRESSED_BYTES}",
            compressed.len()
        ));
    }
    let decoded = bounded_lz4::decompress_size_prepended(
        compressed,
        MAX_CATALOG_NAVIGATION_BYTES,
        "embedded catalog navigation",
    )?;
    let projection = decode_navigation_projection(&decoded)?;
    Ok(projection.matches(expected_stamp).then_some(projection))
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
    read_catalog_navigation_projection_with_timing(path, expected_stamp)
        .map(|loaded| loaded.map(|loaded| loaded.projection))
}

pub fn read_catalog_navigation_projection_with_timing(
    path: &Path,
    expected_stamp: &CatalogStamp,
) -> Result<Option<CatalogNavigationProjectionRead>, String> {
    catalog_load_metrics::record_nav_projection_read();
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("open catalog navigation {}: {e}", path.display())),
    };
    let compressed_len = file
        .metadata()
        .map_err(|e| format!("inspect catalog navigation {}: {e}", path.display()))?
        .len();
    if compressed_len > MAX_CATALOG_NAVIGATION_COMPRESSED_BYTES {
        return Err(format!(
            "catalog navigation {} compressed size {compressed_len} exceeds max {MAX_CATALOG_NAVIGATION_COMPRESSED_BYTES}",
            path.display()
        ));
    }
    let read_started = std::time::Instant::now();
    let bytes = read_bounded(
        file,
        MAX_CATALOG_NAVIGATION_COMPRESSED_BYTES,
        &format!("catalog navigation {}", path.display()),
    )?;
    let file_read_us = read_started.elapsed().as_micros() as u64;
    let decompress_started = std::time::Instant::now();
    let decoded = bounded_lz4::decompress_size_prepended(
        &bytes,
        MAX_CATALOG_NAVIGATION_BYTES,
        "catalog navigation",
    )
    .map_err(|e| format!("decompress catalog navigation {}: {e}", path.display()))?;
    let decompress_us = decompress_started.elapsed().as_micros() as u64;
    let decode_started = std::time::Instant::now();
    let projection = decode_navigation_projection(&decoded)
        .map_err(|e| format!("parse catalog navigation {}: {e}", path.display()))?;
    let decode_us = decode_started.elapsed().as_micros() as u64;
    if !projection.matches(expected_stamp) {
        return Ok(None);
    }
    Ok(Some(CatalogNavigationProjectionRead {
        projection,
        file_read_us,
        decompress_us,
        decode_us,
    }))
}

fn read_bounded(file: File, max_bytes: u64, label: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(
        file.metadata()
            .map(|metadata| metadata.len().min(max_bytes) as usize)
            .unwrap_or(0),
    );
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read {label}: {e}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "{label} actual compressed size {} exceeds max {max_bytes}",
            bytes.len()
        ));
    }
    Ok(bytes)
}

pub fn read_catalog_navigation_snapshot(
    path: &Path,
) -> Result<CatalogNavigationProjection, String> {
    let compressed = std::fs::read(path)
        .map_err(|e| format!("read catalog navigation snapshot {}: {e}", path.display()))?;
    if compressed.len() as u64 > MAX_CATALOG_NAVIGATION_COMPRESSED_BYTES {
        return Err("catalog navigation snapshot exceeds compressed size limit".into());
    }
    let decoded = bounded_lz4::decompress_size_prepended(
        &compressed,
        MAX_CATALOG_NAVIGATION_BYTES,
        "catalog navigation snapshot",
    )?;
    let projection = decode_navigation_projection(&decoded)?;
    let embedded_stamp = CatalogStamp::from_lines(projection.catalog_stamp_lines.clone());
    if !projection.matches(&embedded_stamp) {
        return Err("catalog navigation snapshot has an invalid embedded stamp".into());
    }
    Ok(projection)
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
            systems: catalog
                .systems
                .iter()
                .map(|system| {
                    NavigationSystem::from_system(system, catalog.platform_kind(&system.id))
                })
                .collect(),
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

impl NavigationSystem {
    fn from_system(system: &GameSystemEntry, platform_kind: PlatformKind) -> Self {
        Self {
            id: system.id.clone(),
            title: system.title.clone(),
            count: system.count,
            platform_kind,
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
            players: game.players,
            control: game.control.clone(),
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
            players: game.players,
            control: game.control,
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
    let mut seen = HashSet::<&str>::new();
    let mut plans = Vec::new();
    for game in catalog.games.iter() {
        if !seen.insert(game.mra_path.as_ref()) {
            continue;
        }
        if let Some(plan) = catalog.structured_launch_plan_for_ref(game.mra_path.as_ref()) {
            plans.push(NavigationLaunchPlan::from(plan));
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
    preview_archive_path: &'a str,
    preview_asset_key: &'a str,
    has_preview: bool,
    system_id: &'a str,
    year: Option<u16>,
    manufacturer: &'a str,
    category: &'a str,
    players: Option<u8>,
    control: &'a str,
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
    players: Option<u8>,
    control: Arc<str>,
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
                preview_archive_path: game.preview_archive_path.as_ref(),
                preview_asset_key: game.preview_asset_key.as_ref(),
                has_preview: game.has_preview,
                system_id: game.system_id.as_ref(),
                year: game.year,
                manufacturer: game.manufacturer.as_ref(),
                category: game.category.as_ref(),
                players: game.players,
                control: game.control.as_ref(),
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
        out.push(system.platform_kind.encoded());
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
        write_string(&mut out, game.preview_archive_path)?;
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
        match game.players {
            Some(players) => {
                write_bool(&mut out, true);
                out.push(players);
            }
            None => write_bool(&mut out, false),
        }
        write_string(&mut out, game.control)?;
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
    let stamp_line_count = read_navigation_count(&mut reader, 4, "stamp lines")?;
    let mut catalog_stamp_lines = Vec::new();
    reserve_navigation_vec(&mut catalog_stamp_lines, stamp_line_count, "stamp lines")?;
    for _ in 0..stamp_line_count {
        catalog_stamp_lines.push(reader.read_string()?);
    }
    let system_count = read_navigation_count(&mut reader, 17, "systems")?;
    let mut systems = Vec::new();
    reserve_navigation_vec(&mut systems, system_count, "systems")?;
    for _ in 0..system_count {
        systems.push(NavigationSystem {
            id: reader.read_string()?,
            title: reader.read_string()?,
            count: reader
                .read_u64()?
                .try_into()
                .map_err(|_| "system count too large".to_string())?,
            platform_kind: PlatformKind::from_encoded(reader.read_u8()?)
                .ok_or_else(|| "navigation system platform kind is invalid".to_string())?,
        });
    }
    let launch_default_count = read_navigation_count(&mut reader, 14, "launch defaults")?;
    let mut launch_defaults = HashMap::<String, NavigationLaunchDefault>::new();
    launch_defaults
        .try_reserve(launch_default_count)
        .map_err(|err| {
            format!("allocate navigation launch defaults ({launch_default_count}): {err}")
        })?;
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
    let game_count = read_navigation_count(&mut reader, 32, "games")?;
    let mut game_rows = Vec::new();
    reserve_navigation_vec(&mut game_rows, game_count, "games")?;
    for _ in 0..game_count {
        let title = reader.read_arc_string()?;
        let launch_ref = match reader.read_u8()? {
            NAV_REF_FULL => CompactDecodedGameLaunchRef::Full(reader.read_arc_string()?),
            NAV_REF_PAYLOAD => CompactDecodedGameLaunchRef::PlanIndex(reader.read_u32()? as usize),
            value => {
                return Err(format!(
                    "navigation projection launch ref mode {value} is invalid"
                ));
            }
        };
        let preview_asset_key = reader.read_arc_string()?;
        let preview_archive_path = reader.read_arc_string()?;
        let has_preview = reader.read_bool()?;
        let system_id = reader.read_arc_string()?;
        let year = if reader.read_bool()? {
            Some(reader.read_u16()?)
        } else {
            None
        };
        let manufacturer = reader.read_arc_string()?;
        let category = reader.read_arc_string()?;
        let players = if reader.read_bool()? {
            Some(reader.read_u8()?)
        } else {
            None
        };
        let control = reader.read_arc_string()?;
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
            players,
            control,
            is_new,
        });
    }
    let launch_plan_count = read_navigation_count(&mut reader, 11, "launch plans")?;
    let mut launch_plans = Vec::new();
    reserve_navigation_vec(&mut launch_plans, launch_plan_count, "launch plans")?;
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
    let mut games = Vec::new();
    reserve_navigation_vec(&mut games, game_rows.len(), "decoded games")?;
    let mut launch_refs_by_plan = Vec::new();
    reserve_navigation_vec(
        &mut launch_refs_by_plan,
        launch_plans.len(),
        "decoded launch references",
    )?;
    launch_refs_by_plan.extend(launch_plans.iter().map(|plan| plan.launch_ref.clone()));
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
            players: game.players,
            control: game.control,
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

fn read_navigation_count(
    reader: &mut NavigationBinaryReader<'_>,
    minimum_item_bytes: usize,
    label: &str,
) -> Result<usize, String> {
    let count = reader.read_len()?;
    let max_by_remaining_bytes = reader.remaining() / minimum_item_bytes;
    if count > MAX_CATALOG_NAVIGATION_ITEMS || count > max_by_remaining_bytes {
        return Err(format!(
            "navigation projection {label} count {count} exceeds available bounds"
        ));
    }
    Ok(count)
}

fn reserve_navigation_vec<T>(values: &mut Vec<T>, count: usize, label: &str) -> Result<(), String> {
    values
        .try_reserve_exact(count)
        .map_err(|err| format!("allocate navigation {label} ({count}): {err}"))
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

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
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
    let mut fault_control = crate::fs_fault::NoopDirectResetFaultControl;
    write_bytes_atomically_with_fault_control(final_path, bytes, &mut fault_control)
}

fn write_bytes_atomically_with_fault_control(
    final_path: &Path,
    bytes: &[u8],
    fault_control: &mut dyn crate::fs_fault::DirectResetFaultControl,
) -> Result<(), String> {
    crate::atomic_publish::write_atomically_with_fault_control(
        final_path,
        "catalog navigation",
        "catalog.nav.lz4b",
        Some("catalog.navigation"),
        fault_control,
        |file| file.write_all(bytes),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::arcade_game;

    #[test]
    fn navigation_count_rejects_impossible_and_excessive_allocations() {
        let mut impossible = NavigationBinaryReader::new(&[2, 0, 0, 0]);
        assert!(read_navigation_count(&mut impossible, 4, "fixture").is_err());

        let mut bytes = Vec::with_capacity(MAX_CATALOG_NAVIGATION_ITEMS + 5);
        bytes.extend_from_slice(&((MAX_CATALOG_NAVIGATION_ITEMS + 1) as u32).to_le_bytes());
        bytes.resize(MAX_CATALOG_NAVIGATION_ITEMS + 5, 0);
        let mut excessive = NavigationBinaryReader::new(&bytes);
        assert!(read_navigation_count(&mut excessive, 1, "fixture").is_err());
    }

    fn stamp(lines: &[&str]) -> CatalogStamp {
        CatalogStamp::from_lines(lines.iter().map(|line| line.to_string()).collect())
    }

    fn game(title: &str, launch_ref: &str, system_id: &str) -> ArcadeGameEntry {
        arcade_game(title)
            .path(launch_ref)
            .preview(title.to_ascii_lowercase())
            .system_id(system_id)
            .year(1984)
            .manufacturer("Capcom")
            .players(2)
            .control("joy")
            .build()
    }

    fn projection_catalog() -> ArcadeCatalog {
        let saturn_payload = "/media/fat/games/Saturn/Nights.chd";
        let neogeo_payload = "/media/fat/games/NEOGEO/Pack.zip/Pack/World A-Z/mslug.neo";
        let mut games = vec![
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
        games.push(game(
            "Nights (Shared Launch Ref)",
            &format!("magik-plan:payload:{saturn_payload}"),
            "saturn",
        ));
        games[2].preview_archive_path =
            Arc::from("/media/fat/mister-magik/assets/custom-neogeo-pack.mmlz4b");
        let systems = vec![
            GameSystemEntry {
                id: "arcade".to_string(),
                title: "Arcade".to_string(),
                count: 1,
            },
            GameSystemEntry {
                id: "saturn".to_string(),
                title: "Saturn".to_string(),
                count: 2,
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

    fn owned_launch_plan_extraction_reference(
        catalog: &ArcadeCatalog,
    ) -> Vec<NavigationLaunchPlan> {
        let mut seen = HashSet::new();
        let mut plans = Vec::new();
        for game in catalog.games.iter() {
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

    #[test]
    fn borrowed_launch_plan_extraction_preserves_encoded_bytes() {
        let catalog = projection_catalog();
        let stamp = stamp(&["root /media/fat/games", "core Saturn 123"]);
        let optimized = CatalogNavigationProjection::from_catalog(&catalog, &stamp);
        let mut reference = optimized.clone();
        reference.launch_plans = owned_launch_plan_extraction_reference(&catalog);

        assert_eq!(optimized.launch_plans, reference.launch_plans);
        assert_eq!(
            encode_navigation_projection(&optimized).expect("encode borrowed extraction"),
            encode_navigation_projection(&reference).expect("encode owned extraction reference")
        );
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
        assert_eq!(
            loaded.games[2].preview_archive_path.as_ref(),
            "/media/fat/mister-magik/assets/custom-neogeo-pack.mmlz4b"
        );
        assert_eq!(
            hydrated.games[2].preview_archive_path.as_ref(),
            "/media/fat/mister-magik/assets/custom-neogeo-pack.mmlz4b"
        );
        assert_eq!(hydrated.games[0].players, Some(2));
        assert_eq!(hydrated.games[0].control.as_ref(), "joy");
        assert_eq!(hydrated.games.len(), catalog.games.len());
        assert_eq!(hydrated.systems, catalog.systems);
        assert_eq!(loaded.systems[0].platform_kind, PlatformKind::Arcade);
        assert_eq!(hydrated.platform_kind("saturn"), PlatformKind::Console);
        assert_eq!(hydrated.platform_kind("neogeo"), PlatformKind::Arcade);
        assert_eq!(hydrated.decade_option_count("arcade"), 1);
        assert_eq!(hydrated.manufacturer_option_count("arcade"), 1);
        assert_eq!(hydrated.player_option_count("arcade"), 1);
        assert_eq!(hydrated.control_option_count("arcade"), 1);
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
    fn navigation_snapshot_timing_reports_each_write_phase() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-navigation-timing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp dir");
        let path = root.join("catalog-ready.nav.lz4b");
        let stamp = stamp(&["root /media/fat/games"]);
        let catalog = projection_catalog();

        let timing = write_catalog_navigation_snapshot_with_timing(&path, &catalog, &stamp)
            .expect("write timed snapshot");
        let loaded = read_catalog_navigation_snapshot(&path).expect("read timed snapshot");

        assert_eq!(
            timing.compressed_bytes as u64,
            path.metadata().unwrap().len()
        );
        assert!(timing.encoded_bytes > 0);
        assert!(timing.compressed_bytes > 0);
        assert_eq!(loaded.games.len(), catalog.games.len());
        assert_eq!(loaded.launch_plans.len(), 2);
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
        assert!(
            read_catalog_navigation_projection(&path, &stale_stamp)
                .expect("read stale projection")
                .is_none()
        );
        std::fs::write(&path, b"not-lz4").expect("write corrupt projection");
        assert!(read_catalog_navigation_projection(&path, &current_stamp).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bounded_navigation_read_rejects_bytes_beyond_limit() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-navigation-bounded-read-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp dir");
        let path = root.join("navigation.bin");
        std::fs::write(&path, b"123456").expect("write fixture");

        let error = read_bounded(File::open(&path).expect("open fixture"), 5, "fixture")
            .expect_err("oversized fixture must fail");
        assert!(error.contains("actual compressed size 6 exceeds max 5"));
        assert_eq!(
            read_bounded(File::open(&path).expect("reopen fixture"), 6, "fixture")
                .expect("exact bound"),
            b"123456"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn navigation_projection_rejects_previous_binary_schema() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-navigation-old-schema-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp dir");
        let db = root.join("library.sqlite3");
        let path = navigation_path_for_sqlite(&db);
        let stamp = stamp(&["root current"]);

        write_catalog_navigation_projection_for_catalog(&db, &projection_catalog(), &stamp)
            .expect("write projection");
        let mut bytes = std::fs::read(&path).expect("read current projection");
        let mut decoded =
            lz4_flex::decompress_size_prepended(&bytes).expect("decompress current projection");
        decoded[..8].copy_from_slice(b"MMNAVB7\0");
        bytes = lz4_flex::compress_prepend_size(&decoded);
        std::fs::write(&path, bytes).expect("write old schema projection");

        assert!(read_catalog_navigation_projection(&path, &stamp).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
