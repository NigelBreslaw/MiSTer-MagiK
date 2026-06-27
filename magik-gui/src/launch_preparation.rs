//! Launch-ref classification and materialization before Main handoff.

use crate::{arcade_catalog::LaunchTarget, library_db};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::time::Instant;

const VIRTUAL_LAUNCH_PREFIX: &str = "magik-plan:";
const AMIGAVISION_GAME_LAUNCH_PREFIX: &str = "magik-amigavision:";
const AMIGAVISION_LAUNCHER_REF: &str = "magik-amigavision-launcher";
const AMIGAVISION_MGL_PATH: &str = "/media/fat/_Computer/Amiga.mgl";
const AMIGAVISION_HDF_PATH: &str = "/media/fat/games/Amiga/AmigaVision.hdf";
const AMIGAVISION_SHARED_DIR: &str = "/media/fat/games/Amiga/shared";
const AMIGAVISION_AGS_BOOT: &str = "/media/fat/games/Amiga/shared/ags_boot";

pub fn prepare_launch_ref(launch_ref: &str) -> Result<String, String> {
    if launch_ref.starts_with(VIRTUAL_LAUNCH_PREFIX) {
        Err(format!(
            "structured launch ref must be resolved from catalog before launch: {launch_ref}"
        ))
    } else if launch_ref.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX) {
        materialize_amigavision_game_launch_ref(launch_ref)
    } else if launch_ref == AMIGAVISION_LAUNCHER_REF {
        materialize_amigavision_launcher_ref()
    } else {
        Ok(launch_ref.to_string())
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LaunchPrepBenchScenario {
    Warm,
    Cold,
    PriorityPrewarm,
}

impl LaunchPrepBenchScenario {
    fn from_arg(value: Option<&str>) -> Self {
        match value.unwrap_or("warm").trim().to_ascii_lowercase().as_str() {
            "cold" => Self::Cold,
            "priority-prewarm" | "prewarm" => Self::PriorityPrewarm,
            _ => Self::Warm,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Warm => "warm",
            Self::Cold => "cold",
            Self::PriorityPrewarm => "priority-prewarm",
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
    read_bytes: u64,
    rchar: u64,
    syscr: u64,
    write_bytes: u64,
    wchar: u64,
    syscw: u64,
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
    let catalog = match library_db::load_arcade_catalog_from_sqlite(
        crate::arcade_catalog::DEFAULT_ARCADE_ROOT,
    ) {
        Ok(loaded) => loaded.catalog,
        Err(e) => {
            eprintln!("launch_prep_bench\tfailed\tload catalog: {e}");
            std::process::exit(1);
        }
    };
    let refs = match launch_prep_bench_refs_from_env()
        .or_else(|_| load_default_launch_prep_bench_refs(&catalog))
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
            "launch_prep_bench_summary\t{label}\t{}\tcount=0\terrors=0\tp50_us=0\tp95_us=0\tread_bytes=0\trchar=0\tsyscr=0\twrite_bytes=0\twchar=0\tsyscw=0",
            scenario.label()
        );
        return;
    }

    let mut samples = Vec::with_capacity(refs.len() * iterations);
    let mut errors = 0usize;
    let mut total_read_bytes = 0u64;
    let mut total_rchar = 0u64;
    let mut total_syscr = 0u64;
    let mut total_write_bytes = 0u64;
    let mut total_wchar = 0u64;
    let mut total_syscw = 0u64;
    for iteration in 0..iterations {
        if scenario == LaunchPrepBenchScenario::PriorityPrewarm {
            let before = read_self_proc_io();
            let start = Instant::now();
            let prewarm_us = start.elapsed().as_micros() as u64;
            let after = read_self_proc_io();
            let read_bytes = after.read_bytes.saturating_sub(before.read_bytes);
            let rchar = after.rchar.saturating_sub(before.rchar);
            let syscr = after.syscr.saturating_sub(before.syscr);
            let write_bytes = after.write_bytes.saturating_sub(before.write_bytes);
            let wchar = after.wchar.saturating_sub(before.wchar);
            let syscw = after.syscw.saturating_sub(before.syscw);
            total_read_bytes = total_read_bytes.saturating_add(read_bytes);
            total_rchar = total_rchar.saturating_add(rchar);
            total_syscr = total_syscr.saturating_add(syscr);
            total_write_bytes = total_write_bytes.saturating_add(write_bytes);
            total_wchar = total_wchar.saturating_add(wchar);
            total_syscw = total_syscw.saturating_add(syscw);
            println!(
                "launch_prep_bench_prewarm_tsv\t{label}\t{}\t{iteration}\tstatus=removed\ttotal=0\twritten=0\tunchanged=0\terrors=0\tprewarm_us={prewarm_us}\tread_bytes={read_bytes}\trchar={rchar}\tsyscr={syscr}\twrite_bytes={write_bytes}\twchar={wchar}\tsyscw={syscw}",
                scenario.label()
            );
        }
        for (idx, bench_ref) in refs.iter().enumerate() {
            if scenario == LaunchPrepBenchScenario::Cold {
                prepare_cold_launch_prep_ref(&bench_ref.launch_ref);
            }
            let before = read_self_proc_io();
            let start = Instant::now();
            let result = prepare_launch_bench_ref(&catalog, &bench_ref.launch_ref);
            let prepare_us = start.elapsed().as_micros() as u64;
            let after = read_self_proc_io();
            let read_bytes = after.read_bytes.saturating_sub(before.read_bytes);
            let rchar = after.rchar.saturating_sub(before.rchar);
            let syscr = after.syscr.saturating_sub(before.syscr);
            let write_bytes = after.write_bytes.saturating_sub(before.write_bytes);
            let wchar = after.wchar.saturating_sub(before.wchar);
            let syscw = after.syscw.saturating_sub(before.syscw);
            total_read_bytes = total_read_bytes.saturating_add(read_bytes);
            total_rchar = total_rchar.saturating_add(rchar);
            total_syscr = total_syscr.saturating_add(syscr);
            total_write_bytes = total_write_bytes.saturating_add(write_bytes);
            total_wchar = total_wchar.saturating_add(wchar);
            total_syscw = total_syscw.saturating_add(syscw);
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
                "launch_prep_bench_tsv\t{label}\t{}\t{iteration}\t{idx}\t{}\t{status}\t{prepare_us}\tread_bytes={read_bytes}\trchar={rchar}\tsyscr={syscr}\twrite_bytes={write_bytes}\twchar={wchar}\tsyscw={syscw}\ttarget={}\tref={}",
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
        "launch_prep_bench_summary\t{label}\t{}\tcount={}\terrors={errors}\tp50_us={p50}\tp95_us={p95}\tread_bytes={total_read_bytes}\trchar={total_rchar}\tsyscr={total_syscr}\twrite_bytes={total_write_bytes}\twchar={total_wchar}\tsyscw={total_syscw}",
        scenario.label(),
        samples.len()
    );
}

fn prepare_launch_bench_ref(
    catalog: &crate::arcade_catalog::ArcadeCatalog,
    launch_ref: &str,
) -> Result<String, String> {
    if launch_ref.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX)
        || launch_ref == AMIGAVISION_LAUNCHER_REF
    {
        return prepare_launch_ref(launch_ref);
    }
    Ok(match catalog.launch_target_for_ref(launch_ref) {
        LaunchTarget::Path(path) => path.to_string(),
        LaunchTarget::Structured(plan) => format!("structured:{}", plan.launch_ref),
        LaunchTarget::MissingStructured(launch_ref) => {
            return Err(format!(
                "structured launch plan missing from catalog: {launch_ref}"
            ));
        }
    })
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

fn load_default_launch_prep_bench_refs(
    catalog: &crate::arcade_catalog::ArcadeCatalog,
) -> Result<Vec<LaunchPrepBenchRef>, String> {
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
        for game in catalog
            .system_game_view(system_id)
            .iter()
            .filter(|game| game.mra_path.starts_with(VIRTUAL_LAUNCH_PREFIX))
            .take(virtual_limit)
        {
            refs.push(LaunchPrepBenchRef {
                kind: format!("virtual-{}", game.system_id),
                launch_ref: game.mra_path.to_string(),
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
    if launch_ref.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX) {
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
        if let Some(value) = line.strip_prefix("read_bytes:") {
            counters.read_bytes = value.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("rchar:") {
            counters.rchar = value.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("syscr:") {
            counters.syscr = value.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("write_bytes:") {
            counters.write_bytes = value.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("wchar:") {
            counters.wchar = value.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("syscw:") {
            counters.syscw = value.trim().parse::<u64>().unwrap_or(0);
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
    fn virtual_launch_ref_is_not_materialized_by_launch_prep() {
        let err = prepare_launch_ref("magik-plan:payload-saturn-test")
            .expect_err("virtual launch refs require catalog hydration");

        assert!(err.contains("structured launch ref must be resolved from catalog"));
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
}
