// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Volatile, bounded catalog seed used only for an in-session game return.

use crate::arcade_catalog::{
    ArcadeCatalog, ArcadeGameEntry, GameSystemEntry, LaunchTarget, PlatformKind,
    StructuredLaunchPlan,
};
use mister_magik_catalog::catalog_config::{self, CATALOG_BUILD_VERSION, SCHEMA_VERSION};
use mister_magik_catalog::device_layout::DeviceLayout;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const RETURN_CATALOG_CAPSULE_PATH: &str = "/tmp/mister-magik/launcher-return-catalog.json";
const RETURN_CATALOG_CAPSULE_MAGIC: &[u8; 8] = b"MMRCAP03";
const RETURN_CATALOG_CAPSULE_SCHEMA: u32 = 3;
const RETURN_CATALOG_CAPSULE_MAX_BYTES: u64 = 8 * 1024 * 1024;
const RETURN_CATALOG_CAPSULE_MAX_ROWS: usize = 10_000;
const RETURN_CATALOG_CAPSULE_MAX_SYSTEMS: usize = 4_096;
const RETURN_CATALOG_CAPSULE_MAX_PLANS: usize = 10_000;
const RETURN_CATALOG_CAPSULE_MAX_ROOTS: usize = 64;
const RETURN_CATALOG_CAPSULE_MAX_PATH_MAPS: usize = 64;
const RETURN_CATALOG_CAPSULE_MAX_STRING_BYTES: usize = 4_096;

const MIN_ENCODED_STRING_BYTES: usize = 4;
const MIN_ENCODED_SYSTEM_BYTES: usize = 17;
const MIN_ENCODED_GAME_BYTES: usize = 31;
const MIN_ENCODED_PLAN_BYTES: usize = 26;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CapsuleBinding {
    namespace: String,
    app_dir: String,
    catalog_root: String,
    library_roots: Vec<String>,
    path_map: Vec<(String, String)>,
    catalog_stamp_fingerprint: String,
    catalog_schema: u32,
    catalog_build: u32,
    binary_version: String,
    binary_build: String,
}

impl CapsuleBinding {
    fn current(catalog_root: &Path) -> Result<Self, String> {
        let state = mister_magik_catalog::catalog_state::read(
            &mister_magik_catalog::catalog_state::default_path(),
        )?;
        Self::for_current_generation(catalog_root, &state.stamp.fingerprint_hex())
    }

    fn for_current_generation(
        catalog_root: &Path,
        durable_catalog_fingerprint: &str,
    ) -> Result<Self, String> {
        validate_catalog_fingerprint(durable_catalog_fingerprint)?;
        Ok(Self::for_values(
            DeviceLayout::current(),
            catalog_root,
            catalog_config::library_roots_from_env(),
            catalog_config::library_path_map_from_env()
                .into_iter()
                .map(|rule| (rule.from, rule.to))
                .collect(),
            durable_catalog_fingerprint.to_string(),
        ))
    }

    fn for_values(
        layout: DeviceLayout,
        catalog_root: &Path,
        library_roots: Vec<String>,
        path_map: Vec<(String, String)>,
        stamp: String,
    ) -> Self {
        let namespace = match layout {
            DeviceLayout::Public => "mister-magik-public",
            DeviceLayout::Dev => "mister-magik-dev",
        };
        Self {
            namespace: namespace.to_string(),
            app_dir: layout.app_dir().to_string(),
            catalog_root: catalog_root.to_string_lossy().into_owned(),
            library_roots,
            path_map,
            catalog_stamp_fingerprint: stamp,
            catalog_schema: SCHEMA_VERSION,
            catalog_build: CATALOG_BUILD_VERSION,
            binary_version: crate::build_identity::BuildIdentity::current()
                .package_version
                .to_string(),
            binary_build: crate::build_identity::BuildIdentity::current()
                .build_time
                .to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreparedReturnCatalogCapsule {
    binding: CapsuleBinding,
    collection_id: String,
    return_game_path: String,
    systems: Vec<GameSystemEntry>,
    games: Vec<ArcadeGameEntry>,
    platform_kinds: HashMap<String, PlatformKind>,
    launch_plans: Vec<StructuredLaunchPlan>,
}

#[derive(Debug)]
pub struct TakenReturnCatalogCapsule {
    pub catalog: ArcadeCatalog,
    pub durable_catalog_fingerprint: String,
}

#[derive(Clone, Debug)]
struct ReturnCatalogCapsule {
    binding: CapsuleBinding,
    collection_id: String,
    return_game_path: String,
    systems: Vec<CapsuleSystem>,
    games: Vec<CapsuleGame>,
    launch_plans: Vec<CapsuleLaunchPlan>,
}

#[derive(Clone, Debug)]
struct CapsuleSystem {
    id: String,
    title: String,
    count: usize,
    platform_kind: PlatformKind,
}

#[derive(Clone, Debug)]
struct CapsuleGame {
    title: String,
    mra_path: String,
    preview_archive_path: String,
    preview_asset_key: String,
    has_preview: bool,
    system_id: String,
    year: Option<u16>,
    manufacturer: String,
    category: String,
    players: Option<u8>,
    control: String,
    is_new: bool,
}

#[derive(Clone, Debug)]
struct CapsuleLaunchPlan {
    launch_ref: String,
    title: String,
    system_id: String,
    core_path: String,
    payload_path: String,
    mount_kind: String,
    mount_index: u8,
    delay_secs: u8,
}

pub fn prepare_return_catalog_capsule(
    catalog: &ArcadeCatalog,
    collection_id: &str,
    return_game_path: &str,
    durable_catalog_fingerprint: &str,
) -> Result<PreparedReturnCatalogCapsule, String> {
    let binding =
        CapsuleBinding::for_current_generation(&catalog.root, durable_catalog_fingerprint)?;
    prepare_return_catalog_capsule_inner(catalog, collection_id, return_game_path, binding)
}

#[cfg(test)]
fn prepare_return_catalog_capsule_with_binding(
    catalog: &ArcadeCatalog,
    collection_id: &str,
    return_game_path: &str,
    binding: CapsuleBinding,
) -> Result<PreparedReturnCatalogCapsule, String> {
    prepare_return_catalog_capsule_inner(catalog, collection_id, return_game_path, binding)
}

fn prepare_return_catalog_capsule_inner(
    catalog: &ArcadeCatalog,
    collection_id: &str,
    return_game_path: &str,
    binding: CapsuleBinding,
) -> Result<PreparedReturnCatalogCapsule, String> {
    validate_string("collection id", collection_id)?;
    validate_string("return game path", return_game_path)?;
    let view = catalog.system_game_view(collection_id);
    if view.is_empty() {
        return Err(format!("return collection {collection_id:?} has no rows"));
    }
    if view.len() > RETURN_CATALOG_CAPSULE_MAX_ROWS {
        return Err(format!(
            "return collection has {} rows; limit is {}",
            view.len(),
            RETURN_CATALOG_CAPSULE_MAX_ROWS
        ));
    }
    if catalog.systems.len() > RETURN_CATALOG_CAPSULE_MAX_SYSTEMS {
        return Err(format!(
            "catalog has {} systems; limit is {}",
            catalog.systems.len(),
            RETURN_CATALOG_CAPSULE_MAX_SYSTEMS
        ));
    }

    let games: Vec<_> = view.iter().cloned().collect();
    if !games
        .iter()
        .any(|game| game.mra_path.as_ref() == return_game_path)
    {
        return Err("return game is absent from the current collection".to_string());
    }
    let mut plan_refs = HashSet::new();
    let mut launch_plans = Vec::new();
    for game in &games {
        if let LaunchTarget::Structured(plan) = catalog.launch_target_for_ref(&game.mra_path)
            && plan_refs.insert(plan.launch_ref.clone())
        {
            launch_plans.push(plan);
        }
    }
    if launch_plans.len() > RETURN_CATALOG_CAPSULE_MAX_PLANS {
        return Err("return capsule has too many structured launch plans".to_string());
    }

    let systems = catalog.systems.clone();
    let platform_kinds = systems
        .iter()
        .map(|system| (system.id.clone(), catalog.platform_kind(&system.id)))
        .collect();
    let prepared = PreparedReturnCatalogCapsule {
        binding,
        collection_id: collection_id.to_string(),
        return_game_path: return_game_path.to_string(),
        systems,
        games,
        platform_kinds,
        launch_plans,
    };
    prepared.validate()?;
    Ok(prepared)
}

impl PreparedReturnCatalogCapsule {
    fn validate(&self) -> Result<(), String> {
        validate_binding(&self.binding)?;
        validate_string("collection id", &self.collection_id)?;
        validate_string("return game path", &self.return_game_path)?;
        validate_count(
            "systems",
            self.systems.len(),
            RETURN_CATALOG_CAPSULE_MAX_SYSTEMS,
        )?;
        validate_count("games", self.games.len(), RETURN_CATALOG_CAPSULE_MAX_ROWS)?;
        validate_count(
            "launch plans",
            self.launch_plans.len(),
            RETURN_CATALOG_CAPSULE_MAX_PLANS,
        )?;
        for system in &self.systems {
            validate_string("system id", &system.id)?;
            validate_string("system title", &system.title)?;
        }
        for game in &self.games {
            validate_game(game)?;
        }
        for plan in &self.launch_plans {
            validate_plan(plan)?;
        }
        Ok(())
    }

    fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut writer = CapsuleBinaryWriter::new();
        writer.write_bytes(RETURN_CATALOG_CAPSULE_MAGIC)?;
        writer.write_u32(RETURN_CATALOG_CAPSULE_SCHEMA)?;
        encode_binding(&mut writer, &self.binding)?;
        writer.write_string(&self.collection_id)?;
        writer.write_string(&self.return_game_path)?;

        writer.write_count(self.systems.len(), "systems")?;
        for system in &self.systems {
            writer.write_string(&system.id)?;
            writer.write_string(&system.title)?;
            writer.write_u64(system.count as u64)?;
            writer.write_u8(encode_platform_kind(
                self.platform_kinds
                    .get(&system.id)
                    .copied()
                    .unwrap_or_default(),
            ))?;
        }

        writer.write_count(self.games.len(), "games")?;
        for game in &self.games {
            writer.write_string(&game.title)?;
            writer.write_string(&game.mra_path)?;
            writer.write_string(&game.preview_archive_path)?;
            writer.write_string(&game.preview_asset_key)?;
            writer.write_bool(game.has_preview)?;
            writer.write_string(&game.system_id)?;
            writer.write_bool(game.year.is_some())?;
            if let Some(year) = game.year {
                writer.write_u16(year)?;
            }
            writer.write_string(&game.manufacturer)?;
            writer.write_string(&game.category)?;
            writer.write_bool(game.players.is_some())?;
            if let Some(players) = game.players {
                writer.write_u8(players)?;
            }
            writer.write_string(&game.control)?;
            writer.write_bool(game.is_new)?;
        }

        writer.write_count(self.launch_plans.len(), "launch plans")?;
        for plan in &self.launch_plans {
            writer.write_string(&plan.launch_ref)?;
            writer.write_string(&plan.title)?;
            writer.write_string(&plan.system_id)?;
            writer.write_string(&plan.core_path)?;
            writer.write_string(&plan.payload_path)?;
            writer.write_string(&plan.mount_kind)?;
            writer.write_u8(plan.mount_index)?;
            writer.write_u8(plan.delay_secs)?;
        }
        Ok(writer.finish())
    }
}

pub fn save_return_catalog_capsule(capsule: &PreparedReturnCatalogCapsule) -> Result<(), String> {
    let _pmu = mister_magik_perf_events::sampled_span("launch.return-capsule-save");
    save_return_catalog_capsule_at(Path::new(RETURN_CATALOG_CAPSULE_PATH), capsule)
}

fn save_return_catalog_capsule_at(
    path: &Path,
    capsule: &PreparedReturnCatalogCapsule,
) -> Result<(), String> {
    let encode_pmu = mister_magik_perf_events::sampled_span("launch.return-capsule-encode");
    let bytes = capsule.encode()?;
    drop(encode_pmu);
    let parent = path
        .parent()
        .ok_or_else(|| format!("return capsule path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("create return capsule dir: {e}"))?;
    let tmp = temp_path(path);
    remove_file_quiet(&tmp);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("create return capsule temp: {e}"))?;
    file.write_all(&bytes)
        .map_err(|e| format!("write return capsule temp: {e}"))?;
    file.flush()
        .map_err(|e| format!("flush return capsule temp: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("install return capsule: {e}"))?;
    Ok(())
}

pub fn take_return_catalog_capsule(
    catalog_root: &Path,
    collection_id: &str,
    return_game_path: &str,
) -> Result<TakenReturnCatalogCapsule, String> {
    let expected = match CapsuleBinding::current(catalog_root) {
        Ok(expected) => expected,
        Err(error) => {
            remove_return_catalog_capsule();
            return Err(format!("bind return capsule: {error}"));
        }
    };
    take_return_catalog_capsule_at(
        Path::new(RETURN_CATALOG_CAPSULE_PATH),
        &expected,
        collection_id,
        return_game_path,
    )
}

fn take_return_catalog_capsule_at(
    path: &Path,
    expected: &CapsuleBinding,
    collection_id: &str,
    return_game_path: &str,
) -> Result<TakenReturnCatalogCapsule, String> {
    let result = read_return_catalog_capsule_at(path, expected, collection_id, return_game_path)
        .map(|catalog| TakenReturnCatalogCapsule {
            catalog,
            durable_catalog_fingerprint: expected.catalog_stamp_fingerprint.clone(),
        });
    remove_file_quiet(path);
    remove_file_quiet(&temp_path(path));
    result
}

fn read_return_catalog_capsule_at(
    path: &Path,
    expected: &CapsuleBinding,
    collection_id: &str,
    return_game_path: &str,
) -> Result<ArcadeCatalog, String> {
    let file = File::open(path).map_err(|e| format!("open return capsule: {e}"))?;
    let file_len = file
        .metadata()
        .map_err(|e| format!("stat return capsule: {e}"))?
        .len();
    if file_len > RETURN_CATALOG_CAPSULE_MAX_BYTES {
        return Err("return capsule exceeds byte limit".to_string());
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(file_len as usize)
        .map_err(|e| format!("allocate return capsule read buffer: {e}"))?;
    file.take(RETURN_CATALOG_CAPSULE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read return capsule: {e}"))?;
    if bytes.len() as u64 > RETURN_CATALOG_CAPSULE_MAX_BYTES {
        return Err("return capsule exceeds byte limit".to_string());
    }
    decode_return_catalog_capsule(&bytes, expected, collection_id, return_game_path)
}

fn decode_return_catalog_capsule(
    bytes: &[u8],
    expected: &CapsuleBinding,
    collection_id: &str,
    return_game_path: &str,
) -> Result<ArcadeCatalog, String> {
    let mut reader = CapsuleBinaryReader::new(bytes);
    reader.expect_magic(RETURN_CATALOG_CAPSULE_MAGIC)?;
    let schema = reader.read_u32()?;
    if schema != RETURN_CATALOG_CAPSULE_SCHEMA {
        return Err(format!("return capsule schema {schema} is unsupported"));
    }
    let capsule = ReturnCatalogCapsule {
        binding: decode_binding(&mut reader)?,
        collection_id: reader.read_string("collection id")?,
        return_game_path: reader.read_string("return game path")?,
        systems: decode_systems(&mut reader)?,
        games: decode_games(&mut reader)?,
        launch_plans: decode_launch_plans(&mut reader)?,
    };
    reader.finish()?;
    capsule.into_catalog(expected, collection_id, return_game_path)
}

fn encode_binding(
    writer: &mut CapsuleBinaryWriter,
    binding: &CapsuleBinding,
) -> Result<(), String> {
    writer.write_string(&binding.namespace)?;
    writer.write_string(&binding.app_dir)?;
    writer.write_string(&binding.catalog_root)?;
    writer.write_count(binding.library_roots.len(), "library roots")?;
    for root in &binding.library_roots {
        writer.write_string(root)?;
    }
    writer.write_count(binding.path_map.len(), "path maps")?;
    for (from, to) in &binding.path_map {
        writer.write_string(from)?;
        writer.write_string(to)?;
    }
    writer.write_string(&binding.catalog_stamp_fingerprint)?;
    writer.write_u32(binding.catalog_schema)?;
    writer.write_u32(binding.catalog_build)?;
    writer.write_string(&binding.binary_version)?;
    writer.write_string(&binding.binary_build)?;
    Ok(())
}

fn decode_binding(reader: &mut CapsuleBinaryReader<'_>) -> Result<CapsuleBinding, String> {
    let namespace = reader.read_string("namespace")?;
    let app_dir = reader.read_string("app dir")?;
    let catalog_root = reader.read_string("catalog root")?;
    let root_count = reader.read_count(
        "library roots",
        RETURN_CATALOG_CAPSULE_MAX_ROOTS,
        MIN_ENCODED_STRING_BYTES,
    )?;
    let mut library_roots = Vec::new();
    reserve_vec(&mut library_roots, root_count, "library roots")?;
    for _ in 0..root_count {
        library_roots.push(reader.read_string("library root")?);
    }
    let path_map_count = reader.read_count(
        "path maps",
        RETURN_CATALOG_CAPSULE_MAX_PATH_MAPS,
        MIN_ENCODED_STRING_BYTES * 2,
    )?;
    let mut path_map = Vec::new();
    reserve_vec(&mut path_map, path_map_count, "path maps")?;
    for _ in 0..path_map_count {
        path_map.push((
            reader.read_string("path map source")?,
            reader.read_string("path map destination")?,
        ));
    }
    Ok(CapsuleBinding {
        namespace,
        app_dir,
        catalog_root,
        library_roots,
        path_map,
        catalog_stamp_fingerprint: reader.read_string("catalog stamp")?,
        catalog_schema: reader.read_u32()?,
        catalog_build: reader.read_u32()?,
        binary_version: reader.read_string("binary version")?,
        binary_build: reader.read_string("binary build")?,
    })
}

fn decode_systems(reader: &mut CapsuleBinaryReader<'_>) -> Result<Vec<CapsuleSystem>, String> {
    let count = reader.read_count(
        "systems",
        RETURN_CATALOG_CAPSULE_MAX_SYSTEMS,
        MIN_ENCODED_SYSTEM_BYTES,
    )?;
    let mut systems = Vec::new();
    reserve_vec(&mut systems, count, "systems")?;
    for _ in 0..count {
        systems.push(CapsuleSystem {
            id: reader.read_string("system id")?,
            title: reader.read_string("system title")?,
            count: reader
                .read_u64()?
                .try_into()
                .map_err(|_| "system count is too large".to_string())?,
            platform_kind: decode_platform_kind(reader.read_u8()?)?,
        });
    }
    Ok(systems)
}

fn decode_games(reader: &mut CapsuleBinaryReader<'_>) -> Result<Vec<CapsuleGame>, String> {
    let count = reader.read_count(
        "games",
        RETURN_CATALOG_CAPSULE_MAX_ROWS,
        MIN_ENCODED_GAME_BYTES,
    )?;
    if count == 0 {
        return Err("return capsule has no game rows".to_string());
    }
    let mut games = Vec::new();
    reserve_vec(&mut games, count, "games")?;
    for _ in 0..count {
        let title = reader.read_string("game title")?;
        let mra_path = reader.read_string("game path")?;
        let preview_archive_path = reader.read_string("preview archive path")?;
        let preview_asset_key = reader.read_string("preview asset key")?;
        let has_preview = reader.read_bool()?;
        let system_id = reader.read_string("game system id")?;
        let year = reader.read_bool()?.then(|| reader.read_u16()).transpose()?;
        let manufacturer = reader.read_string("manufacturer")?;
        let category = reader.read_string("category")?;
        let players = reader.read_bool()?.then(|| reader.read_u8()).transpose()?;
        let control = reader.read_string("control")?;
        let is_new = reader.read_bool()?;
        games.push(CapsuleGame {
            title,
            mra_path,
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
    Ok(games)
}

fn decode_launch_plans(
    reader: &mut CapsuleBinaryReader<'_>,
) -> Result<Vec<CapsuleLaunchPlan>, String> {
    let count = reader.read_count(
        "launch plans",
        RETURN_CATALOG_CAPSULE_MAX_PLANS,
        MIN_ENCODED_PLAN_BYTES,
    )?;
    let mut plans = Vec::new();
    reserve_vec(&mut plans, count, "launch plans")?;
    for _ in 0..count {
        plans.push(CapsuleLaunchPlan {
            launch_ref: reader.read_string("launch ref")?,
            title: reader.read_string("launch title")?,
            system_id: reader.read_string("launch system id")?,
            core_path: reader.read_string("core path")?,
            payload_path: reader.read_string("payload path")?,
            mount_kind: reader.read_string("mount kind")?,
            mount_index: reader.read_u8()?,
            delay_secs: reader.read_u8()?,
        });
    }
    Ok(plans)
}

impl ReturnCatalogCapsule {
    fn into_catalog(
        self,
        expected: &CapsuleBinding,
        collection_id: &str,
        return_game_path: &str,
    ) -> Result<ArcadeCatalog, String> {
        if &self.binding != expected {
            return Err("return capsule binding mismatch".to_string());
        }
        if self.collection_id != collection_id || self.return_game_path != return_game_path {
            return Err("return capsule selection mismatch".to_string());
        }
        validate_binding(&self.binding)?;

        let mut platform_kinds = HashMap::new();
        platform_kinds
            .try_reserve(self.systems.len())
            .map_err(|e| format!("allocate platform kinds: {e}"))?;
        let systems = self
            .systems
            .into_iter()
            .map(|system| {
                platform_kinds.insert(system.id.clone(), system.platform_kind);
                GameSystemEntry {
                    id: system.id,
                    title: system.title,
                    count: system.count,
                }
            })
            .collect();
        let games = self
            .games
            .into_iter()
            .map(CapsuleGame::into_game)
            .collect::<Vec<_>>();
        if !games
            .iter()
            .any(|game| game.mra_path.as_ref() == return_game_path)
        {
            return Err("return capsule omits selected game".to_string());
        }
        let plans = self
            .launch_plans
            .into_iter()
            .map(CapsuleLaunchPlan::into_plan)
            .collect();
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds(
            PathBuf::from(&self.binding.catalog_root),
            games,
            systems,
            plans,
            platform_kinds,
        );
        if catalog.system_game_view(collection_id).is_empty() {
            return Err("return capsule cannot reconstruct collection".to_string());
        }
        Ok(catalog)
    }
}

impl CapsuleGame {
    fn into_game(self) -> ArcadeGameEntry {
        ArcadeGameEntry {
            title: Arc::from(self.title),
            mra_path: Arc::from(self.mra_path),
            preview_archive_path: Arc::from(self.preview_archive_path),
            preview_asset_key: Arc::from(self.preview_asset_key),
            has_preview: self.has_preview,
            system_id: Arc::from(self.system_id),
            year: self.year,
            manufacturer: Arc::from(self.manufacturer),
            category: Arc::from(self.category),
            players: self.players,
            control: Arc::from(self.control),
            is_new: self.is_new,
        }
    }
}

impl CapsuleLaunchPlan {
    fn into_plan(self) -> StructuredLaunchPlan {
        StructuredLaunchPlan {
            launch_ref: Arc::from(self.launch_ref),
            title: Arc::from(self.title),
            system_id: Arc::from(self.system_id),
            core_path: Arc::from(self.core_path),
            payload_path: Arc::from(self.payload_path),
            mount_kind: Arc::from(self.mount_kind),
            mount_index: self.mount_index,
            delay_secs: self.delay_secs,
        }
    }
}

struct CapsuleBinaryWriter {
    bytes: Vec<u8>,
}

impl CapsuleBinaryWriter {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }

    fn reserve(&mut self, additional: usize) -> Result<(), String> {
        let new_len = self
            .bytes
            .len()
            .checked_add(additional)
            .ok_or_else(|| "return capsule encoded size overflow".to_string())?;
        if new_len as u64 > RETURN_CATALOG_CAPSULE_MAX_BYTES {
            return Err(format!(
                "return capsule would exceed {} bytes",
                RETURN_CATALOG_CAPSULE_MAX_BYTES
            ));
        }
        self.bytes
            .try_reserve_exact(additional)
            .map_err(|e| format!("allocate return capsule encoding: {e}"))
    }

    fn write_bytes(&mut self, value: &[u8]) -> Result<(), String> {
        self.reserve(value.len())?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    fn write_u8(&mut self, value: u8) -> Result<(), String> {
        self.write_bytes(&[value])
    }

    fn write_u16(&mut self, value: u16) -> Result<(), String> {
        self.write_bytes(&value.to_le_bytes())
    }

    fn write_u32(&mut self, value: u32) -> Result<(), String> {
        self.write_bytes(&value.to_le_bytes())
    }

    fn write_u64(&mut self, value: u64) -> Result<(), String> {
        self.write_bytes(&value.to_le_bytes())
    }

    fn write_bool(&mut self, value: bool) -> Result<(), String> {
        self.write_u8(u8::from(value))
    }

    fn write_count(&mut self, value: usize, label: &str) -> Result<(), String> {
        let encoded: u32 = value
            .try_into()
            .map_err(|_| format!("{label} count is too large"))?;
        self.write_u32(encoded)
    }

    fn write_string(&mut self, value: &str) -> Result<(), String> {
        validate_string("encoded string", value)?;
        self.write_count(value.len(), "string byte")?;
        self.write_bytes(value.as_bytes())
    }
}

struct CapsuleBinaryReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> CapsuleBinaryReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn read_bytes(&mut self, len: usize, label: &str) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| format!("{label} length overflow"))?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| format!("return capsule is truncated while reading {label}"))?;
        self.offset = end;
        Ok(value)
    }

    fn expect_magic(&mut self, expected: &[u8]) -> Result<(), String> {
        if self.read_bytes(expected.len(), "magic")? != expected {
            return Err("return capsule magic mismatch".to_string());
        }
        Ok(())
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        Ok(self.read_bytes(1, "u8")?[0])
    }

    fn read_u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(
            self.read_bytes(2, "u16")?
                .try_into()
                .expect("bounded u16 slice"),
        ))
    }

    fn read_u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.read_bytes(4, "u32")?
                .try_into()
                .expect("bounded u32 slice"),
        ))
    }

    fn read_u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(
            self.read_bytes(8, "u64")?
                .try_into()
                .expect("bounded u64 slice"),
        ))
    }

    fn read_bool(&mut self) -> Result<bool, String> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(format!("return capsule boolean {value} is invalid")),
        }
    }

    fn read_count(
        &mut self,
        label: &str,
        max: usize,
        minimum_encoded_bytes: usize,
    ) -> Result<usize, String> {
        let count = self.read_u32()? as usize;
        if count > max {
            return Err(format!(
                "return capsule {label} count {count} exceeds {max}"
            ));
        }
        let minimum = count
            .checked_mul(minimum_encoded_bytes)
            .ok_or_else(|| format!("return capsule {label} minimum size overflow"))?;
        if minimum > self.remaining() {
            return Err(format!(
                "return capsule {label} count {count} cannot fit in {} remaining bytes",
                self.remaining()
            ));
        }
        Ok(count)
    }

    fn read_string(&mut self, label: &str) -> Result<String, String> {
        let len = self.read_u32()? as usize;
        if len > RETURN_CATALOG_CAPSULE_MAX_STRING_BYTES {
            return Err(format!(
                "return capsule {label} is {len} bytes; limit is {}",
                RETURN_CATALOG_CAPSULE_MAX_STRING_BYTES
            ));
        }
        let bytes = self.read_bytes(len, label)?;
        let value = std::str::from_utf8(bytes)
            .map_err(|e| format!("return capsule {label} is not UTF-8: {e}"))?;
        let mut owned = String::new();
        owned
            .try_reserve_exact(len)
            .map_err(|e| format!("allocate return capsule {label}: {e}"))?;
        owned.push_str(value);
        Ok(owned)
    }

    fn finish(self) -> Result<(), String> {
        if self.offset != self.bytes.len() {
            return Err(format!(
                "return capsule has {} trailing bytes",
                self.bytes.len() - self.offset
            ));
        }
        Ok(())
    }
}

fn reserve_vec<T>(vec: &mut Vec<T>, count: usize, label: &str) -> Result<(), String> {
    vec.try_reserve_exact(count)
        .map_err(|e| format!("allocate return capsule {label} ({count}): {e}"))
}

fn encode_platform_kind(kind: PlatformKind) -> u8 {
    match kind {
        PlatformKind::Unknown => 0,
        PlatformKind::Arcade => 1,
        PlatformKind::Console => 2,
        PlatformKind::Handheld => 3,
        PlatformKind::Computer => 4,
    }
}

fn decode_platform_kind(value: u8) -> Result<PlatformKind, String> {
    match value {
        0 => Ok(PlatformKind::Unknown),
        1 => Ok(PlatformKind::Arcade),
        2 => Ok(PlatformKind::Console),
        3 => Ok(PlatformKind::Handheld),
        4 => Ok(PlatformKind::Computer),
        _ => Err(format!("return capsule platform kind {value} is invalid")),
    }
}

fn validate_binding(binding: &CapsuleBinding) -> Result<(), String> {
    validate_count(
        "library roots",
        binding.library_roots.len(),
        RETURN_CATALOG_CAPSULE_MAX_ROOTS,
    )?;
    validate_count(
        "path maps",
        binding.path_map.len(),
        RETURN_CATALOG_CAPSULE_MAX_PATH_MAPS,
    )?;
    for (label, value) in [
        ("namespace", binding.namespace.as_str()),
        ("app dir", binding.app_dir.as_str()),
        ("catalog root", binding.catalog_root.as_str()),
        ("catalog stamp", binding.catalog_stamp_fingerprint.as_str()),
        ("binary version", binding.binary_version.as_str()),
        ("binary build", binding.binary_build.as_str()),
    ] {
        validate_string(label, value)?;
    }
    validate_catalog_fingerprint(&binding.catalog_stamp_fingerprint)?;
    for root in &binding.library_roots {
        validate_string("library root", root)?;
    }
    for (from, to) in &binding.path_map {
        validate_string("path map source", from)?;
        validate_string("path map destination", to)?;
    }
    Ok(())
}

fn validate_game(game: &ArcadeGameEntry) -> Result<(), String> {
    for (label, value) in [
        ("game title", game.title.as_ref()),
        ("game path", game.mra_path.as_ref()),
        ("preview archive path", game.preview_archive_path.as_ref()),
        ("preview asset key", game.preview_asset_key.as_ref()),
        ("game system id", game.system_id.as_ref()),
        ("manufacturer", game.manufacturer.as_ref()),
        ("category", game.category.as_ref()),
        ("control", game.control.as_ref()),
    ] {
        validate_string(label, value)?;
    }
    Ok(())
}

fn validate_plan(plan: &StructuredLaunchPlan) -> Result<(), String> {
    for (label, value) in [
        ("launch ref", plan.launch_ref.as_ref()),
        ("launch title", plan.title.as_ref()),
        ("launch system id", plan.system_id.as_ref()),
        ("core path", plan.core_path.as_ref()),
        ("payload path", plan.payload_path.as_ref()),
        ("mount kind", plan.mount_kind.as_ref()),
    ] {
        validate_string(label, value)?;
    }
    Ok(())
}

fn validate_count(label: &str, value: usize, max: usize) -> Result<(), String> {
    if value > max {
        return Err(format!(
            "return capsule {label} count {value} exceeds {max}"
        ));
    }
    Ok(())
}

fn validate_catalog_fingerprint(value: &str) -> Result<(), String> {
    validate_string("catalog fingerprint", value)?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("catalog fingerprint must be non-empty hexadecimal text".to_string());
    }
    Ok(())
}

fn validate_string(label: &str, value: &str) -> Result<(), String> {
    if value.len() > RETURN_CATALOG_CAPSULE_MAX_STRING_BYTES {
        return Err(format!(
            "{label} is {} bytes; limit is {}",
            value.len(),
            RETURN_CATALOG_CAPSULE_MAX_STRING_BYTES
        ));
    }
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("launcher-return-catalog.json");
    tmp.set_file_name(format!("{name}.tmp"));
    tmp
}

fn remove_file_quiet(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

pub fn remove_return_catalog_capsule() {
    let path = Path::new(RETURN_CATALOG_CAPSULE_PATH);
    remove_file_quiet(path);
    remove_file_quiet(&temp_path(path));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::arcade_game;

    fn unique_temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-{label}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    fn binding(root: &Path) -> CapsuleBinding {
        CapsuleBinding::for_values(
            DeviceLayout::Dev,
            root,
            vec!["/media/fat/_Arcade".to_string()],
            Vec::new(),
            "0123456789abcdef".to_string(),
        )
    }

    fn catalog(root: &Path) -> ArcadeCatalog {
        let games = vec![
            arcade_game("One")
                .path("/games/one.mra")
                .preview("one.png")
                .build(),
            arcade_game("Two").path("/games/two.mra").build(),
            arcade_game("Other")
                .path("/games/other.rom")
                .system_id("nes")
                .preview("other.png")
                .build(),
        ];
        ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds(
            root.to_path_buf(),
            games,
            vec![
                GameSystemEntry {
                    id: "arcade".to_string(),
                    title: "Arcade".to_string(),
                    count: 2,
                },
                GameSystemEntry {
                    id: "nes".to_string(),
                    title: "NES".to_string(),
                    count: 1,
                },
            ],
            Vec::new(),
            HashMap::from([
                ("arcade".to_string(), PlatformKind::Arcade),
                ("nes".to_string(), PlatformKind::Console),
            ]),
        )
    }

    fn encoded_capsule(root: &Path) -> Vec<u8> {
        prepare_return_catalog_capsule_with_binding(
            &catalog(root),
            "arcade",
            "/games/two.mra",
            binding(root),
        )
        .expect("prepare")
        .encode()
        .expect("encode")
    }

    #[test]
    fn capsule_round_trip_restores_only_current_collection_and_selection() {
        let root = unique_temp_dir("return-capsule-roundtrip");
        let path = root.join("capsule.bin");
        let source = catalog(&root);
        let prepared = prepare_return_catalog_capsule_with_binding(
            &source,
            "arcade",
            "/games/two.mra",
            binding(&root),
        )
        .expect("prepare");
        save_return_catalog_capsule_at(&path, &prepared).expect("save");
        let restored =
            take_return_catalog_capsule_at(&path, &binding(&root), "arcade", "/games/two.mra")
                .expect("take");
        assert_eq!(
            restored.durable_catalog_fingerprint,
            binding(&root).catalog_stamp_fingerprint
        );
        assert_eq!(restored.catalog.len(), 2);
        assert_eq!(restored.catalog.system_game_view("arcade").len(), 2);
        assert!(
            restored
                .catalog
                .games
                .iter()
                .any(|game| game.mra_path.as_ref() == "/games/two.mra")
        );
        assert!(!path.exists());

        let repeated = prepare_return_catalog_capsule_with_binding(
            &restored.catalog,
            "arcade",
            "/games/two.mra",
            binding(&root),
        )
        .expect("prepare repeated");
        save_return_catalog_capsule_at(&path, &repeated).expect("save repeated");
        let repeated =
            take_return_catalog_capsule_at(&path, &binding(&root), "arcade", "/games/two.mra")
                .expect("take repeated");
        assert_eq!(repeated.catalog.system_game_view("arcade").len(), 2);
        assert_eq!(
            repeated.durable_catalog_fingerprint,
            binding(&root).catalog_stamp_fingerprint
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capsule_catalog_preserves_exact_launch_return_selection() {
        let root = unique_temp_dir("return-capsule-selection");
        let bytes = encoded_capsule(&root);
        let restored =
            decode_return_catalog_capsule(&bytes, &binding(&root), "arcade", "/games/two.mra")
                .expect("decode");
        let source = catalog(&root);
        let mut launched_nav = crate::launcher::LauncherNav::new();
        launched_nav.sync_launcher_taxonomy(&source);
        assert!(launched_nav.open_system(&source, "arcade"));
        let state =
            crate::launcher::capture_launch_return_state(&launched_nav, &source, "/games/two.mra")
                .expect("return state");
        let mut returned_nav = crate::launcher::LauncherNav::new();
        assert!(crate::launcher::apply_launch_return_state(
            &mut returned_nav,
            &restored,
            state
        ));
        assert_eq!(returned_nav.arcade.selected, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capsule_rejects_row_string_root_and_map_bounds() {
        let root = unique_temp_dir("return-capsule-bounds");
        let mut source = catalog(&root);
        Arc::make_mut(&mut source.games)[0].title =
            Arc::from("x".repeat(RETURN_CATALOG_CAPSULE_MAX_STRING_BYTES + 1));
        assert!(
            prepare_return_catalog_capsule_with_binding(
                &source,
                "arcade",
                "/games/one.mra",
                binding(&root),
            )
            .is_err()
        );

        let mut too_many_roots = binding(&root);
        too_many_roots.library_roots =
            vec!["/root".to_string(); RETURN_CATALOG_CAPSULE_MAX_ROOTS + 1];
        assert!(validate_binding(&too_many_roots).is_err());
        let mut too_many_maps = binding(&root);
        too_many_maps.path_map = vec![
            ("/from".to_string(), "/to".to_string());
            RETURN_CATALOG_CAPSULE_MAX_PATH_MAPS + 1
        ];
        assert!(validate_binding(&too_many_maps).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malicious_counts_and_strings_reject_before_reservation() {
        let count = u32::MAX.to_le_bytes();
        assert!(
            CapsuleBinaryReader::new(&count)
                .read_count(
                    "games",
                    RETURN_CATALOG_CAPSULE_MAX_ROWS,
                    MIN_ENCODED_GAME_BYTES
                )
                .is_err()
        );
        assert!(
            CapsuleBinaryReader::new(&count)
                .read_count(
                    "library roots",
                    RETURN_CATALOG_CAPSULE_MAX_ROOTS,
                    MIN_ENCODED_STRING_BYTES,
                )
                .is_err()
        );
        assert!(
            CapsuleBinaryReader::new(&count)
                .read_count(
                    "path maps",
                    RETURN_CATALOG_CAPSULE_MAX_PATH_MAPS,
                    MIN_ENCODED_STRING_BYTES * 2,
                )
                .is_err()
        );

        let declared = ((RETURN_CATALOG_CAPSULE_MAX_STRING_BYTES + 1) as u32).to_le_bytes();
        assert!(
            CapsuleBinaryReader::new(&declared)
                .read_string("malicious string")
                .is_err()
        );

        let exact = "x".repeat(RETURN_CATALOG_CAPSULE_MAX_STRING_BYTES);
        let mut writer = CapsuleBinaryWriter::new();
        writer.write_string(&exact).expect("encode exact limit");
        assert_eq!(
            CapsuleBinaryReader::new(&writer.finish())
                .read_string("exact string")
                .expect("decode exact limit"),
            exact
        );

        let plausible_but_truncated = 10u32.to_le_bytes();
        assert!(
            CapsuleBinaryReader::new(&plausible_but_truncated)
                .read_count(
                    "systems",
                    RETURN_CATALOG_CAPSULE_MAX_SYSTEMS,
                    MIN_ENCODED_SYSTEM_BYTES
                )
                .is_err()
        );
    }

    #[test]
    fn binary_parser_rejects_magic_schema_trailing_truncated_utf8_bool_and_enum() {
        let root = unique_temp_dir("return-capsule-parser");
        let expected = binding(&root);
        let mut bytes = encoded_capsule(&root);
        bytes[0] ^= 0xff;
        assert!(
            decode_return_catalog_capsule(&bytes, &expected, "arcade", "/games/two.mra").is_err()
        );

        let mut bytes = encoded_capsule(&root);
        bytes[RETURN_CATALOG_CAPSULE_MAGIC.len()..RETURN_CATALOG_CAPSULE_MAGIC.len() + 4]
            .copy_from_slice(&999u32.to_le_bytes());
        assert!(
            decode_return_catalog_capsule(&bytes, &expected, "arcade", "/games/two.mra").is_err()
        );

        let mut bytes = encoded_capsule(&root);
        bytes.push(0);
        assert!(
            decode_return_catalog_capsule(&bytes, &expected, "arcade", "/games/two.mra").is_err()
        );

        let bytes = encoded_capsule(&root);
        assert!(
            decode_return_catalog_capsule(
                &bytes[..bytes.len() - 1],
                &expected,
                "arcade",
                "/games/two.mra"
            )
            .is_err()
        );

        assert!(CapsuleBinaryReader::new(&[2]).read_bool().is_err());
        assert!(decode_platform_kind(255).is_err());
        let invalid_utf8 = [1, 0, 0, 0, 0xff];
        assert!(
            CapsuleBinaryReader::new(&invalid_utf8)
                .read_string("invalid utf8")
                .is_err()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_stamp_or_layout_is_consumed_and_rejected() {
        let root = unique_temp_dir("return-capsule-stale");
        let path = root.join("capsule.bin");
        let source = catalog(&root);
        let prepared = prepare_return_catalog_capsule_with_binding(
            &source,
            "arcade",
            "/games/one.mra",
            binding(&root),
        )
        .expect("prepare");
        save_return_catalog_capsule_at(&path, &prepared).expect("save");
        let mut stale = binding(&root);
        stale.catalog_stamp_fingerprint = "ffffffffffffffff".to_string();
        assert!(take_return_catalog_capsule_at(&path, &stale, "arcade", "/games/one.mra").is_err());
        assert!(!path.exists());

        save_return_catalog_capsule_at(&path, &prepared).expect("save");
        let mut other_layout = binding(&root);
        other_layout.namespace = "mister-magik-public".to_string();
        assert!(
            take_return_catalog_capsule_at(&path, &other_layout, "arcade", "/games/one.mra")
                .is_err()
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_capsule_is_consumed_for_exact_fallback() {
        let root = unique_temp_dir("return-capsule-corrupt");
        let path = root.join("capsule.bin");
        fs::write(&path, b"{bad").expect("write");
        assert!(
            take_return_catalog_capsule_at(&path, &binding(&root), "arcade", "/games/one.mra")
                .is_err()
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }
}
