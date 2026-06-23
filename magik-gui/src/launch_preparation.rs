//! Launch-ref classification and materialization before Main handoff.

use crate::library_db;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

const VIRTUAL_LAUNCH_CACHE_DIR: &str = "/media/fat/mister-magik/launch-cache";
const VIRTUAL_LAUNCH_CACHE_STAMP_FILE: &str = ".virtual-launch-cache.json";
const VIRTUAL_LAUNCH_CACHE_STAMP_SCHEMA: u32 = 1;
const VIRTUAL_LAUNCH_CACHE_FORMAT_VERSION: u32 = 1;
const VIRTUAL_LAUNCH_PREFIX: &str = "magik-plan:";
const AMIGAVISION_GAME_LAUNCH_PREFIX: &str = "magik-amigavision:";
const AMIGAVISION_LAUNCHER_REF: &str = "magik-amigavision-launcher";
const AMIGAVISION_MGL_PATH: &str = "/media/fat/_Computer/Amiga.mgl";
const AMIGAVISION_HDF_PATH: &str = "/media/fat/games/Amiga/AmigaVision.hdf";
const AMIGAVISION_SHARED_DIR: &str = "/media/fat/games/Amiga/shared";
const AMIGAVISION_AGS_BOOT: &str = "/media/fat/games/Amiga/shared/ags_boot";
const VIRTUAL_LAUNCH_CACHE_SLUG_BYTES: usize = 80;
const FNV128_OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const FNV128_PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VirtualLaunchCacheSummary {
    pub total: usize,
    pub written: usize,
    pub unchanged: usize,
    pub errors: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct VirtualLaunchCacheStamp {
    schema: u32,
    format_version: u32,
    plan_count: usize,
    fingerprint: String,
}

pub fn prepare_launch_ref(launch_ref: &str) -> Result<String, String> {
    if launch_ref.starts_with(VIRTUAL_LAUNCH_PREFIX) {
        prepare_virtual_launch_ref(launch_ref)
    } else if launch_ref.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX) {
        materialize_amigavision_game_launch_ref(launch_ref)
    } else if launch_ref == AMIGAVISION_LAUNCHER_REF {
        materialize_amigavision_launcher_ref()
    } else {
        Ok(launch_ref.to_string())
    }
}

pub fn materialize_virtual_launch_cache_from_default_db() -> VirtualLaunchCacheSummary {
    let plans = match library_db::load_virtual_launch_plans() {
        Ok(plans) => plans,
        Err(e) => {
            eprintln!("virtual launch cache load failed: {e}");
            return VirtualLaunchCacheSummary {
                errors: 1,
                ..VirtualLaunchCacheSummary::default()
            };
        }
    };
    materialize_virtual_launch_plans_at(&plans, &virtual_launch_cache_dir())
}

fn prepare_virtual_launch_ref(launch_ref: &str) -> Result<String, String> {
    let dir = virtual_launch_cache_dir();
    prepare_virtual_launch_ref_at(launch_ref, &dir)
}

fn prepare_virtual_launch_ref_at(launch_ref: &str, dir: &Path) -> Result<String, String> {
    if let Some(path) = warm_virtual_launch_path_at(launch_ref, dir) {
        return Ok(path.display().to_string());
    }
    materialize_virtual_launch_ref_at(launch_ref, dir)
}

fn materialize_virtual_launch_ref_at(launch_ref: &str, dir: &Path) -> Result<String, String> {
    let plan = library_db::load_virtual_launch_plan(launch_ref)?
        .ok_or_else(|| format!("virtual launch plan not found: {launch_ref}"))?;
    let (path, _) = materialize_virtual_launch_plan_at(&plan, dir)?;
    Ok(path.display().to_string())
}

fn materialize_virtual_launch_plans_at(
    plans: &[library_db::VirtualLaunchPlan],
    dir: &Path,
) -> VirtualLaunchCacheSummary {
    let expected_stamp = virtual_launch_cache_stamp(plans);
    if virtual_launch_cache_stamp_matches(dir, &expected_stamp) {
        return VirtualLaunchCacheSummary {
            total: plans.len(),
            unchanged: plans.len(),
            ..VirtualLaunchCacheSummary::default()
        };
    }

    let mut summary = VirtualLaunchCacheSummary {
        total: plans.len(),
        ..VirtualLaunchCacheSummary::default()
    };
    for plan in plans {
        match materialize_virtual_launch_plan_at(plan, dir) {
            Ok((_, true)) => summary.written += 1,
            Ok((_, false)) => summary.unchanged += 1,
            Err(e) => {
                summary.errors += 1;
                eprintln!("virtual launch cache materialize failed: {e}");
            }
        }
    }
    if summary.errors == 0 {
        if let Err(e) = write_virtual_launch_cache_stamp(dir, &expected_stamp) {
            summary.errors += 1;
            eprintln!("virtual launch cache stamp write failed: {e}");
        }
    }
    summary
}

fn virtual_launch_cache_stamp(plans: &[library_db::VirtualLaunchPlan]) -> VirtualLaunchCacheStamp {
    VirtualLaunchCacheStamp {
        schema: VIRTUAL_LAUNCH_CACHE_STAMP_SCHEMA,
        format_version: VIRTUAL_LAUNCH_CACHE_FORMAT_VERSION,
        plan_count: plans.len(),
        fingerprint: format!("{:032x}", virtual_launch_cache_fingerprint(plans)),
    }
}

fn virtual_launch_cache_fingerprint(plans: &[library_db::VirtualLaunchPlan]) -> u128 {
    let mut entries = plans
        .iter()
        .map(|plan| {
            (
                virtual_launch_cache_basename(&plan.launch_ref),
                virtual_mgl_content(plan),
            )
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let mut hash = FNV128_OFFSET;
    hash = fnv128_update(hash, &VIRTUAL_LAUNCH_CACHE_FORMAT_VERSION.to_le_bytes());
    hash = fnv128_update(hash, &(entries.len() as u64).to_le_bytes());
    for (basename, content) in entries {
        hash = fnv128_update(hash, basename.as_bytes());
        hash = fnv128_update(hash, &[0]);
        hash = fnv128_update(hash, content.as_bytes());
        hash = fnv128_update(hash, &[0xff]);
    }
    hash
}

fn virtual_launch_cache_stamp_matches(dir: &Path, expected: &VirtualLaunchCacheStamp) -> bool {
    std::fs::read_to_string(virtual_launch_cache_stamp_path(dir))
        .ok()
        .and_then(|text| serde_json::from_str::<VirtualLaunchCacheStamp>(&text).ok())
        .is_some_and(|stored| stored == *expected)
}

fn write_virtual_launch_cache_stamp(
    dir: &Path,
    stamp: &VirtualLaunchCacheStamp,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create virtual launch cache: {e}"))?;
    let text = serde_json::to_string(stamp).map_err(|e| format!("serialize stamp: {e}"))?;
    fs::write(virtual_launch_cache_stamp_path(dir), format!("{text}\n"))
        .map_err(|e| format!("write virtual launch cache stamp: {e}"))
}

fn virtual_launch_cache_stamp_path(dir: &Path) -> PathBuf {
    dir.join(VIRTUAL_LAUNCH_CACHE_STAMP_FILE)
}

fn materialize_virtual_launch_plan_at(
    plan: &library_db::VirtualLaunchPlan,
    dir: &Path,
) -> Result<(PathBuf, bool), String> {
    if plan.payload_path.trim().is_empty() {
        return Err(format!(
            "virtual launch plan has no payload: {}",
            plan.launch_ref
        ));
    }
    fs::create_dir_all(dir).map_err(|e| format!("create virtual launch cache: {e}"))?;
    let path = virtual_launch_path_at(&plan.launch_ref, dir);
    let content = virtual_mgl_content(plan);
    let should_write = fs::read_to_string(&path)
        .map(|existing| existing != content)
        .unwrap_or(true);
    if should_write {
        fs::write(&path, content).map_err(|e| {
            format!(
                "write virtual launch mgl path={} launch_ref={} hash={:032x}: {e}",
                path.display(),
                plan.launch_ref,
                stable_launch_ref_hash(&plan.launch_ref)
            )
        })?;
    }
    Ok((path, should_write))
}

fn warm_virtual_launch_path_at(launch_ref: &str, dir: &Path) -> Option<PathBuf> {
    let path = virtual_launch_path_at(launch_ref, dir);
    path.is_file().then_some(path)
}

fn virtual_launch_path_at(launch_ref: &str, dir: &Path) -> PathBuf {
    dir.join(virtual_launch_cache_basename(launch_ref))
}

fn virtual_launch_cache_dir() -> PathBuf {
    PathBuf::from(VIRTUAL_LAUNCH_CACHE_DIR)
}

fn materialize_amigavision_launcher_ref() -> Result<String, String> {
    materialize_amigavision_launcher_ref_at(
        Path::new(AMIGAVISION_MGL_PATH),
        Path::new(AMIGAVISION_HDF_PATH),
        Path::new(AMIGAVISION_SHARED_DIR),
        Path::new(AMIGAVISION_AGS_BOOT),
    )
}

fn materialize_amigavision_game_launch_ref(launch_ref: &str) -> Result<String, String> {
    let encoded = launch_ref
        .strip_prefix(AMIGAVISION_GAME_LAUNCH_PREFIX)
        .ok_or_else(|| format!("invalid AmigaVision launch ref: {launch_ref}"))?;
    let title = decode_launch_component(encoded)?;
    materialize_amigavision_game_launch_ref_at(
        &title,
        Path::new(AMIGAVISION_MGL_PATH),
        Path::new(AMIGAVISION_HDF_PATH),
        Path::new(AMIGAVISION_SHARED_DIR),
        Path::new(AMIGAVISION_AGS_BOOT),
    )
}

fn materialize_amigavision_launcher_ref_at(
    mgl_path: &Path,
    hdf_path: &Path,
    shared_dir: &Path,
    ags_boot_path: &Path,
) -> Result<String, String> {
    validate_amigavision_install(mgl_path, hdf_path)?;
    fs::create_dir_all(shared_dir).map_err(|e| format!("create AmigaVision shared dir: {e}"))?;
    match fs::remove_file(ags_boot_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove stale AmigaVision ags_boot: {e}")),
    }
    Ok(mgl_path.display().to_string())
}

fn materialize_amigavision_game_launch_ref_at(
    title: &str,
    mgl_path: &Path,
    hdf_path: &Path,
    shared_dir: &Path,
    ags_boot_path: &Path,
) -> Result<String, String> {
    validate_amigavision_install(mgl_path, hdf_path)?;
    fs::create_dir_all(shared_dir).map_err(|e| format!("create AmigaVision shared dir: {e}"))?;
    let content = format!("{title}\n");
    let should_write = fs::read_to_string(ags_boot_path)
        .map(|existing| existing != content)
        .unwrap_or(true);
    if should_write {
        fs::write(ags_boot_path, content)
            .map_err(|e| format!("write AmigaVision ags_boot: {e}"))?;
    }
    Ok(mgl_path.display().to_string())
}

fn validate_amigavision_install(mgl_path: &Path, hdf_path: &Path) -> Result<(), String> {
    if !mgl_path.is_file() {
        return Err(format!(
            "AmigaVision launcher is not installed: {}",
            mgl_path.display()
        ));
    }
    if !hdf_path.is_file() {
        return Err(format!(
            "AmigaVision HDF is not installed: {}. Extract the AmigaVision MiSTer archive first.",
            hdf_path.display()
        ));
    }
    Ok(())
}

fn virtual_mgl_content(plan: &library_db::VirtualLaunchPlan) -> String {
    let file_type = match plan.mount_kind.as_str() {
        "load-file" => "f",
        "mount-image" => "s",
        _ => "s",
    };
    format!(
        concat!(
            "<mistergamedescription>\n",
            "  <name>{}</name>\n",
            "  <rbf>{}</rbf>\n",
            "  <file delay=\"{}\" type=\"{}\" index=\"{}\" path=\"{}\"/>\n",
            "</mistergamedescription>\n"
        ),
        xml_escape(&plan.title),
        xml_escape(&plan.core_path),
        plan.mount_delay_secs,
        file_type,
        plan.mount_index,
        xml_escape(&plan.payload_path)
    )
}

fn decode_launch_component(value: &str) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(value.len());
    let input = value.as_bytes();
    let mut idx = 0usize;
    while idx < input.len() {
        if input[idx] == b'%' {
            if idx + 2 >= input.len() {
                return Err("invalid percent escape in launch ref".to_string());
            }
            let hi = hex_value(input[idx + 1])
                .ok_or_else(|| "invalid percent escape in launch ref".to_string())?;
            let lo = hex_value(input[idx + 2])
                .ok_or_else(|| "invalid percent escape in launch ref".to_string())?;
            bytes.push((hi << 4) | lo);
            idx += 3;
        } else {
            bytes.push(input[idx]);
            idx += 1;
        }
    }
    String::from_utf8(bytes).map_err(|e| format!("invalid UTF-8 in launch ref: {e}"))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn virtual_launch_cache_basename(launch_ref: &str) -> String {
    // The full launch ref may be a long path-derived identity. Keep cache
    // filenames short for exFAT/FUSE while preserving identity in the hash.
    let slug = launch_ref_slug(launch_ref);
    let hash = stable_launch_ref_hash(launch_ref);
    format!("virtual-{slug}-{hash:032x}.mgl")
}

fn launch_ref_slug(launch_ref: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in launch_ref.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if mapped == '-' {
            if slug.is_empty() || last_dash {
                continue;
            }
            last_dash = true;
        } else {
            last_dash = false;
        }
        slug.push(mapped);
        if slug.len() >= VIRTUAL_LAUNCH_CACHE_SLUG_BYTES {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "ref".to_string()
    } else {
        slug
    }
}

fn stable_launch_ref_hash(launch_ref: &str) -> u128 {
    fnv128_update(FNV128_OFFSET, launch_ref.as_bytes())
}

fn fnv128_update(mut hash: u128, bytes: &[u8]) -> u128 {
    for byte in bytes {
        hash ^= u128::from(*byte);
        hash = hash.wrapping_mul(FNV128_PRIME);
    }
    hash
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LaunchPrepBenchScenario {
    Warm,
    Cold,
}

impl LaunchPrepBenchScenario {
    fn from_arg(value: Option<&str>) -> Self {
        match value.unwrap_or("warm").trim().to_ascii_lowercase().as_str() {
            "cold" => Self::Cold,
            _ => Self::Warm,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Warm => "warm",
            Self::Cold => "cold",
        }
    }
}

#[derive(Clone, Debug)]
struct LaunchPrepBenchRef {
    kind: String,
    launch_ref: String,
}

#[derive(Clone, Copy, Debug, Default)]
struct ProcIoCounters {
    write_bytes: u64,
    wchar: u64,
}

pub fn run_launch_prep_bench() {
    let args: Vec<String> = std::env::args().collect();
    let label = std::env::var("MISTER_LAUNCH_PREP_LABEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| args.get(2).cloned())
        .unwrap_or_else(|| "launch-prep".to_string());
    let scenario = LaunchPrepBenchScenario::from_arg(args.get(3).map(String::as_str));
    let iterations = std::env::var("MISTER_LAUNCH_PREP_ITERATIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| args.get(4).and_then(|value| value.parse::<usize>().ok()))
        .unwrap_or(5)
        .max(1);
    let refs = match launch_prep_bench_refs_from_env()
        .or_else(|_| load_default_launch_prep_bench_refs())
    {
        Ok(refs) => refs,
        Err(e) => {
            eprintln!("launch_prep_bench\tfailed\t{e}");
            std::process::exit(1);
        }
    };
    println!(
        "launch_prep_bench label={label} scenario={} iterations={} refs={}",
        scenario.label(),
        iterations,
        refs.len()
    );
    if refs.is_empty() {
        println!(
            "launch_prep_bench_summary\t{label}\t{}\tcount=0\terrors=0\tp50_us=0\tp95_us=0\twrite_bytes=0\twchar=0",
            scenario.label()
        );
        return;
    }
    if scenario == LaunchPrepBenchScenario::Warm {
        for bench_ref in &refs {
            let _ = prepare_launch_ref(&bench_ref.launch_ref);
        }
    }

    let mut samples = Vec::with_capacity(refs.len() * iterations);
    let mut errors = 0usize;
    let mut total_write_bytes = 0u64;
    let mut total_wchar = 0u64;
    for iteration in 0..iterations {
        for (idx, bench_ref) in refs.iter().enumerate() {
            if scenario == LaunchPrepBenchScenario::Cold {
                prepare_cold_launch_prep_ref(&bench_ref.launch_ref);
            }
            let before = read_self_proc_io();
            let start = Instant::now();
            let result = prepare_launch_ref(&bench_ref.launch_ref);
            let prepare_us = start.elapsed().as_micros() as u64;
            let after = read_self_proc_io();
            let write_bytes = after.write_bytes.saturating_sub(before.write_bytes);
            let wchar = after.wchar.saturating_sub(before.wchar);
            total_write_bytes = total_write_bytes.saturating_add(write_bytes);
            total_wchar = total_wchar.saturating_add(wchar);
            let (status, target) = match result {
                Ok(target) => ("ok", target),
                Err(e) => {
                    errors += 1;
                    ("error", e)
                }
            };
            if status == "ok" {
                samples.push(prepare_us);
            }
            println!(
                "launch_prep_bench_tsv\t{label}\t{}\t{iteration}\t{idx}\t{}\t{status}\t{prepare_us}\twrite_bytes={write_bytes}\twchar={wchar}\ttarget={}\tref={}",
                scenario.label(),
                bench_ref.kind,
                target,
                bench_ref.launch_ref
            );
        }
    }
    samples.sort_unstable();
    let p50 = percentile_sample(&samples, 0.50);
    let p95 = percentile_sample(&samples, 0.95);
    println!(
        "launch_prep_bench_summary\t{label}\t{}\tcount={}\terrors={errors}\tp50_us={p50}\tp95_us={p95}\twrite_bytes={total_write_bytes}\twchar={total_wchar}",
        scenario.label(),
        samples.len()
    );
}

fn launch_prep_bench_refs_from_env() -> Result<Vec<LaunchPrepBenchRef>, String> {
    let Ok(value) = std::env::var("MISTER_LAUNCH_PREP_REFS") else {
        return Err("MISTER_LAUNCH_PREP_REFS unset".to_string());
    };
    let refs = value
        .split('|')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|launch_ref| LaunchPrepBenchRef {
            kind: launch_prep_kind(launch_ref).to_string(),
            launch_ref: launch_ref.to_string(),
        })
        .collect();
    Ok(refs)
}

fn load_default_launch_prep_bench_refs() -> Result<Vec<LaunchPrepBenchRef>, String> {
    let virtual_limit = std::env::var("MISTER_LAUNCH_PREP_VIRTUAL_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(8);
    let amigavision_limit = std::env::var("MISTER_LAUNCH_PREP_AMIGAVISION_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(4);
    let mut refs = Vec::new();
    let virtual_systems = std::env::var("MISTER_LAUNCH_PREP_VIRTUAL_SYSTEMS")
        .unwrap_or_else(|_| "neogeo|saturn|snes".to_string());
    for system_id in virtual_systems
        .split('|')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        for plan in library_db::load_virtual_launch_plans_for_system(system_id, virtual_limit)? {
            refs.push(LaunchPrepBenchRef {
                kind: format!("virtual-{}", plan.system_id),
                launch_ref: plan.launch_ref,
            });
        }
        if refs
            .iter()
            .any(|bench_ref| bench_ref.kind.starts_with("virtual-"))
        {
            break;
        }
    }
    for launch_ref in library_db::load_amigavision_launch_refs(amigavision_limit)? {
        refs.push(LaunchPrepBenchRef {
            kind: "amigavision".to_string(),
            launch_ref,
        });
    }
    Ok(refs)
}

fn launch_prep_kind(launch_ref: &str) -> &'static str {
    if launch_ref.starts_with(VIRTUAL_LAUNCH_PREFIX) {
        "virtual"
    } else if launch_ref.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX) {
        "amigavision"
    } else {
        "direct"
    }
}

fn prepare_cold_launch_prep_ref(launch_ref: &str) {
    if launch_ref.starts_with(VIRTUAL_LAUNCH_PREFIX) {
        let path = virtual_launch_path_at(launch_ref, Path::new(VIRTUAL_LAUNCH_CACHE_DIR));
        let _ = fs::remove_file(path);
    } else if launch_ref.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX) {
        let _ = fs::remove_file(AMIGAVISION_AGS_BOOT);
    }
}

fn read_self_proc_io() -> ProcIoCounters {
    let mut contents = String::new();
    if File::open("/proc/self/io")
        .and_then(|mut file| file.read_to_string(&mut contents))
        .is_err()
    {
        return ProcIoCounters::default();
    }
    let mut counters = ProcIoCounters::default();
    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("write_bytes:") {
            counters.write_bytes = value.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("wchar:") {
            counters.wchar = value.trim().parse::<u64>().unwrap_or(0);
        }
    }
    counters
}

fn percentile_sample(sorted: &[u64], percentile: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len().saturating_sub(1)) as f64 * percentile).ceil() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn virtual_plan(launch_ref: &str) -> library_db::VirtualLaunchPlan {
        library_db::VirtualLaunchPlan {
            launch_ref: launch_ref.to_string(),
            title: "NiGHTS & Dreams".to_string(),
            system_id: "saturn".to_string(),
            core_path: "_Console/Saturn".to_string(),
            payload_path: "/media/fat/games/Saturn/Nights.chd".to_string(),
            mount_kind: "mount-image".to_string(),
            mount_index: 0,
            mount_delay_secs: 1,
        }
    }

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("mister-magik-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[test]
    fn direct_launch_ref_prepares_without_materialization() {
        assert_eq!(
            prepare_launch_ref("/media/fat/_Arcade/test.mra").expect("prepare direct"),
            "/media/fat/_Arcade/test.mra"
        );
    }

    #[test]
    fn amigavision_game_launch_ref_writes_ags_boot() {
        let root = unique_temp_dir("amigavision-launch");
        let mgl = root.join("_Computer/Amiga.mgl");
        let hdf = root.join("games/Amiga/AmigaVision.hdf");
        let shared = root.join("games/Amiga/shared");
        let ags_boot = shared.join("ags_boot");
        std::fs::create_dir_all(mgl.parent().unwrap()).expect("create mgl dir");
        std::fs::create_dir_all(hdf.parent().unwrap()).expect("create hdf dir");
        std::fs::write(&mgl, "<mistergamedescription/>").expect("write mgl");
        std::fs::write(&hdf, "hdf").expect("write hdf");

        let target = materialize_amigavision_game_launch_ref_at(
            "4th & Inches (OCS)[en]",
            &mgl,
            &hdf,
            &shared,
            &ags_boot,
        )
        .expect("materialize AmigaVision game");

        assert_eq!(target, mgl.display().to_string());
        assert_eq!(
            std::fs::read_to_string(&ags_boot).expect("read ags_boot"),
            "4th & Inches (OCS)[en]\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_same_title_launch_skips_rewrite() {
        let root = unique_temp_dir("amigavision-launch-same-title");
        let mgl = root.join("_Computer/Amiga.mgl");
        let hdf = root.join("games/Amiga/AmigaVision.hdf");
        let shared = root.join("games/Amiga/shared");
        let ags_boot = shared.join("ags_boot");
        std::fs::create_dir_all(mgl.parent().unwrap()).expect("create mgl dir");
        std::fs::create_dir_all(&shared).expect("create shared dir");
        std::fs::write(&mgl, "<mistergamedescription/>").expect("write mgl");
        std::fs::write(&hdf, "hdf").expect("write hdf");
        std::fs::write(&ags_boot, "Agony\n").expect("write ags_boot");
        let mut permissions = std::fs::metadata(&ags_boot)
            .expect("stat ags_boot")
            .permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&ags_boot, permissions).expect("make ags_boot read-only");

        let target =
            materialize_amigavision_game_launch_ref_at("Agony", &mgl, &hdf, &shared, &ags_boot)
                .expect("same-title launch should not rewrite ags_boot");

        assert_eq!(target, mgl.display().to_string());
        assert_eq!(
            std::fs::read_to_string(&ags_boot).expect("read ags_boot"),
            "Agony\n"
        );
        let mut permissions = std::fs::metadata(&ags_boot)
            .expect("stat ags_boot")
            .permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&ags_boot, permissions).expect("restore ags_boot writable");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_launcher_ref_removes_stale_ags_boot() {
        let root = unique_temp_dir("amigavision-launcher");
        let mgl = root.join("_Computer/Amiga.mgl");
        let hdf = root.join("games/Amiga/AmigaVision.hdf");
        let shared = root.join("games/Amiga/shared");
        let ags_boot = shared.join("ags_boot");
        std::fs::create_dir_all(mgl.parent().unwrap()).expect("create mgl dir");
        std::fs::create_dir_all(&shared).expect("create shared dir");
        std::fs::write(&mgl, "<mistergamedescription/>").expect("write mgl");
        std::fs::write(&hdf, "hdf").expect("write hdf");
        std::fs::write(&ags_boot, "Agony\n").expect("write stale ags_boot");

        let target = materialize_amigavision_launcher_ref_at(&mgl, &hdf, &shared, &ags_boot)
            .expect("materialize AmigaVision launcher");

        assert_eq!(target, mgl.display().to_string());
        assert!(!ags_boot.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_launch_ref_reports_missing_hdf() {
        let root = unique_temp_dir("amigavision-missing-hdf");
        let mgl = root.join("_Computer/Amiga.mgl");
        let hdf = root.join("games/Amiga/AmigaVision.hdf");
        let shared = root.join("games/Amiga/shared");
        let ags_boot = shared.join("ags_boot");
        std::fs::create_dir_all(mgl.parent().unwrap()).expect("create mgl dir");
        std::fs::write(&mgl, "<mistergamedescription/>").expect("write mgl");

        let err =
            materialize_amigavision_game_launch_ref_at("Agony", &mgl, &hdf, &shared, &ags_boot)
                .expect_err("missing HDF should fail");

        assert!(err.contains("AmigaVision HDF is not installed"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn percent_decodes_amigavision_launch_title() {
        assert_eq!(
            decode_launch_component("4th%20%26%20Inches%20%28OCS%29%5Ben%5D")
                .expect("decode title"),
            "4th & Inches (OCS)[en]"
        );
    }

    #[test]
    fn virtual_mgl_content_mounts_payload_with_core_path() {
        let plan = virtual_plan("magik-plan:payload-saturn-test");

        let content = virtual_mgl_content(&plan);

        assert!(content.contains("<rbf>_Console/Saturn</rbf>"));
        assert!(content.contains("type=\"s\" index=\"0\""));
        assert!(content.contains("path=\"/media/fat/games/Saturn/Nights.chd\""));
        assert!(content.contains("<name>NiGHTS &amp; Dreams</name>"));
    }

    #[test]
    fn virtual_launch_path_is_bounded_for_long_path_derived_refs() {
        let root = unique_temp_dir("virtual-launch-long-ref");
        let launch_ref = concat!(
            "magik-plan:payload:/media/fat/games/GBA/",
            "Crash & Spyro Superpack - Spyro Orange - The Cortex Conspiracy + ",
            "Crash Bandicoot Purple - Ripto's Rampage (USA)/",
            "Crash & Spyro Superpack - Spyro Orange - The Cortex Conspiracy + ",
            "Crash Bandicoot Purple - Ripto's Rampage (USA).gba"
        );
        let mut plan = virtual_plan(launch_ref);
        plan.payload_path = "/media/fat/games/GBA/Crash Spyro.gba".to_string();

        let (path, written) =
            materialize_virtual_launch_plan_at(&plan, &root).expect("materialize long ref");

        assert!(written);
        let basename = path.file_name().unwrap().to_string_lossy();
        assert!(basename.len() <= 255, "{basename}");
        assert!(basename.starts_with("virtual-magik-plan-payload-media-fat-games-gba-crash-"));
        assert!(basename.ends_with(".mgl"));
        assert!(path.is_file());
        assert_eq!(
            std::fs::read_to_string(&path).expect("read virtual mgl"),
            virtual_mgl_content(&plan)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn virtual_launch_cache_basename_hash_distinguishes_matching_slugs() {
        let prefix = format!("magik-plan:{}", "A".repeat(120));
        let first = virtual_launch_cache_basename(&format!("{prefix}/one.gba"));
        let second = virtual_launch_cache_basename(&format!("{prefix}/two.gba"));

        assert_ne!(first, second);
        assert!(first.starts_with("virtual-magik-plan-"));
        assert!(second.starts_with("virtual-magik-plan-"));
        assert!(first.len() <= 255);
        assert!(second.len() <= 255);
    }

    #[test]
    fn virtual_launch_cache_basename_uses_ref_for_empty_slug() {
        let basename = virtual_launch_cache_basename("::::");

        assert!(basename.starts_with("virtual-ref-"));
        assert!(basename.ends_with(".mgl"));
        assert_eq!("virtual-ref-".len() + 32 + ".mgl".len(), basename.len());
    }

    #[test]
    fn warmed_virtual_launch_ref_performs_no_write_or_db_lookup() {
        let root = unique_temp_dir("virtual-launch-warm");
        let plan = virtual_plan("magik-plan:payload-saturn-test");
        let (path, written) =
            materialize_virtual_launch_plan_at(&plan, &root).expect("materialize virtual launch");
        assert!(written);
        let before = std::fs::read_to_string(&path).expect("read virtual mgl");

        let target = prepare_virtual_launch_ref_at(&plan.launch_ref, &root)
            .expect("warm virtual launch should resolve from file");

        assert_eq!(target, path.display().to_string());
        assert_eq!(
            std::fs::read_to_string(&path).expect("read virtual mgl after prepare"),
            before
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stale_virtual_launch_cache_is_refreshed_before_launch() {
        let root = unique_temp_dir("virtual-launch-stale");
        let plan = virtual_plan("magik-plan:payload-saturn-test");
        std::fs::create_dir_all(&root).expect("create virtual launch cache");
        let path = virtual_launch_path_at(&plan.launch_ref, &root);
        std::fs::write(&path, "<stale/>").expect("write stale virtual mgl");

        let summary = materialize_virtual_launch_plans_at(std::slice::from_ref(&plan), &root);

        assert_eq!(
            summary,
            VirtualLaunchCacheSummary {
                total: 1,
                written: 1,
                unchanged: 0,
                errors: 0,
            }
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read refreshed virtual mgl"),
            virtual_mgl_content(&plan)
        );
        let target = prepare_virtual_launch_ref_at(&plan.launch_ref, &root)
            .expect("warm virtual launch should resolve refreshed file");
        assert_eq!(target, path.display().to_string());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn matching_virtual_launch_cache_stamp_skips_per_file_reads() {
        let root = unique_temp_dir("virtual-launch-stamp-hit");
        let plan = virtual_plan("magik-plan:payload-saturn-test");
        let path = virtual_launch_path_at(&plan.launch_ref, &root);

        let first = materialize_virtual_launch_plans_at(std::slice::from_ref(&plan), &root);
        assert_eq!(
            first,
            VirtualLaunchCacheSummary {
                total: 1,
                written: 1,
                unchanged: 0,
                errors: 0,
            }
        );
        assert!(virtual_launch_cache_stamp_path(&root).is_file());

        std::fs::write(&path, "<manual-edit/>").expect("edit cached virtual mgl");
        let second = materialize_virtual_launch_plans_at(std::slice::from_ref(&plan), &root);

        assert_eq!(
            second,
            VirtualLaunchCacheSummary {
                total: 1,
                written: 0,
                unchanged: 1,
                errors: 0,
            }
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read cached virtual mgl"),
            "<manual-edit/>"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn changed_virtual_launch_content_invalidates_cache_stamp() {
        let root = unique_temp_dir("virtual-launch-stamp-content");
        let mut plan = virtual_plan("magik-plan:payload-saturn-test");
        materialize_virtual_launch_plans_at(std::slice::from_ref(&plan), &root);
        let path = virtual_launch_path_at(&plan.launch_ref, &root);

        plan.payload_path = "/media/fat/games/Saturn/Changed.chd".to_string();
        let summary = materialize_virtual_launch_plans_at(std::slice::from_ref(&plan), &root);

        assert_eq!(
            summary,
            VirtualLaunchCacheSummary {
                total: 1,
                written: 1,
                unchanged: 0,
                errors: 0,
            }
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read refreshed virtual mgl"),
            virtual_mgl_content(&plan)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn changed_virtual_launch_plan_set_invalidates_cache_stamp() {
        let root = unique_temp_dir("virtual-launch-stamp-count");
        let first = virtual_plan("magik-plan:payload-saturn-test");
        materialize_virtual_launch_plans_at(std::slice::from_ref(&first), &root);
        let second = virtual_plan("magik-plan:payload-saturn-extra");

        let summary = materialize_virtual_launch_plans_at(&[first.clone(), second.clone()], &root);

        assert_eq!(
            summary,
            VirtualLaunchCacheSummary {
                total: 2,
                written: 1,
                unchanged: 1,
                errors: 0,
            }
        );
        assert!(virtual_launch_path_at(&first.launch_ref, &root).is_file());
        assert!(virtual_launch_path_at(&second.launch_ref, &root).is_file());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn virtual_launch_cache_errors_do_not_write_stamp() {
        let root = unique_temp_dir("virtual-launch-stamp-error");
        let mut plan = virtual_plan("magik-plan:payload-saturn-test");
        plan.payload_path.clear();

        let summary = materialize_virtual_launch_plans_at(std::slice::from_ref(&plan), &root);

        assert_eq!(
            summary,
            VirtualLaunchCacheSummary {
                total: 1,
                written: 0,
                unchanged: 0,
                errors: 1,
            }
        );
        assert!(!virtual_launch_cache_stamp_path(&root).exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
