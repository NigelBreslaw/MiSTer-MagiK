//! Native MiSTer MagiK framebuffer frontend.
//!
//! Subcommands:
//!   Production:
//!     ui [scene] [secs]  Slint UI (default `launcher`, infinite when secs=0)
//!     early-black        route a black launcher framebuffer before full UI
//!     library-refresh    build/update the SQLite library cache
//!     request-library-rebuild
//!                        write rebuild-on-next-boot marker for fault tests
//!     toggle-simple-joystick-setting
//!                        toggle settings.json simple joystick flag for fault tests
//!     reset-delete-database
//!                        delete catalog DB/projections for fault tests
//!     reset-delete-screenshot-packs
//!                        delete screenshot media artifacts for fault tests
//!   Diagnostics:
//!     read               print live video mode + fb params
//!     vsync-probe        print per-frame vsync/fallback pacing diagnostics
//!     cpu-profile-smoke  burn CPU and verify profiler SVG output
//!     hidden-fb-copy-bench
//!                        benchmark RGB565 copies into hidden framebuffer slots
//!     library-sql        inspect the SQLite library cache without sqlite3(1)
//!     hbmame-metadata-from-library
//!                        build supplemental HBMAME metadata from parsed MRA parents
//!   Bench tools (`--features bench-tools`):
//!     media-bench-download
//!                        benchmark screenshot pack downloads and variant decoding
//!     media-bench-save   benchmark screenshot pack save/publish paths
//!     preview-pack-bench benchmark screenshot pack entry access/decode timings
//!     preview-index-refresh-bench
//!                        update DB preview flags from screenshot pack indexes
//!     input              gamepad log / sniff / calibrate
//!   Benchmarks:
//!     scenes             list Slint scene names
//!   Experiments:
//!     effects            list framebuffer effect benchmark names
//!     camera-effects     list classic camera/background effect labels
//!     sprite-effects     list classic sprite/object effect labels
//!     text-effects       list classic game/Amiga text effect labels
//!     raster-effects     list classic raster/palette effect labels
//!     transition-effects list classic screen transition effect labels
//!     preview-transitions list screenshot transition labels
//!     effect-bench       run framebuffer effect benchmarks
//!     library-scan-bench benchmark build, import, cached load, stamp check
//!     launch-prep-bench  benchmark launch-ref preparation without core launch
//!
//! Game/core launch requests must go through MiSTer_MagiK supervision.
//!
//! See docs/architecture.md for display routing and boot handoff; see
//! magik-gui/BUILD.md for toolchain details.

#![allow(dead_code)]

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

mod arcade_button_overrides;
mod arcade_list_renderer;
mod artifact_publish;
mod bitmap_text;
mod boot_analytics;
mod cpu_profile;
mod display_config;
#[cfg(mister_experiments)]
mod experiments;
mod fallible_log;
mod fpga;
#[cfg(feature = "ui")]
mod frame_profile;
mod input;
mod launch_preparation;
mod launcher;
#[cfg(feature = "bench-tools")]
mod media_bench_download;
#[cfg(feature = "bench-tools")]
mod media_bench_save;
mod media_pack_save;
mod memory_pressure;
mod mr_audio;
#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
mod preview_pack_bench;
mod preview_state;
mod runtime_status;
mod screenshot_transitions;
mod settings;
#[cfg(test)]
mod test_support;
mod ui_display;
#[cfg(mister_experiments)]
mod ui_effect_bench;
mod ui_runner;
mod video_i420;
#[cfg(feature = "video")]
mod video_player;
mod vt;

pub use mister_magik_fb::{
    arcade_catalog, command_args, controller_db, input_repeat, input_state, library_db,
    media_update, preview_worker, setup_nav,
};

use fpga::{Fpga, UIO_GET_FB_PAR, UIO_GET_VRES};
use mister_magik_fb::framebuffer::format::{production_label, rgb565_stride_bytes};
#[cfg(feature = "diagnostics")]
use mister_magik_fb::framebuffer::hidden::{HiddenRgb565BufferIndex, HiddenRgb565Framebuffer};
use mister_magik_fb::framebuffer::mapped::MappedRgb565Framebuffer;
use mister_magik_fb::framebuffer::ownership::DisplayOwnerLock;
use mister_magik_fb::framebuffer::route::LauncherFramebufferRoute;
use mister_magik_fb::framebuffer::vsync::{VsyncPacer, VsyncWaitStatus};
use ui_display::UiDisplayPlan;
use ui_runner::ui_boot::{detect_runtime_display_geometry_for_plan, settle_boot_black_frame};

const MISTER_BIN: &str = "/media/fat/MiSTer_MagiK";
const DEFAULT_PROCESS_LOCK_PATH: &str = "/tmp/mister-magik/process.lock";
fn main() {
    let args: Vec<String> = std::env::args().collect();
    mister_magik_fb::crash_report::install_panic_hook(args.clone());
    boot_analytics::event("process_start", format!("args={}", args.join(" ")));

    if args.len() >= 2 {
        if command_args::should_handoff_to_mister(&args[1]) {
            exec_mister(&args);
        }
        if command_args::is_launchable_arg(&args[1]) {
            reject_direct_launch_arg(&args[1]);
        }
    }
    if command_args::needs_explicit_command(&args) {
        crate::ui_errln!("missing command (use: {})", command_args::command_usage());
        std::process::exit(2);
    }

    let cmd = command_args::resolve_command(&args);

    if cmd != "library-sql" {
        crate::ui_logln!("mister-magik-fb [{cmd}] (arch={})", std::env::consts::ARCH);
    }

    let command = command_args::find_command(&cmd).unwrap_or_else(|| unknown_command(&cmd));
    let _process_lock = if command_args::requires_process_exclusive(&cmd) {
        match MagikProcessLock::acquire_default() {
            Ok(ProcessLockState::Acquired(lock)) => {
                crate::ui_logln!("process_lock\tacquired\t{}", lock.path().display());
                Some(lock)
            }
            Ok(ProcessLockState::Active { pid }) => {
                if cmd == "library-refresh" {
                    crate::ui_logln!("library_refresh\tskipped\tactive_pid={pid}");
                    return;
                }
                crate::ui_errln!("process_lock\trefused\tactive_pid={pid}");
                std::process::exit(13);
            }
            Err(error) => {
                crate::ui_errln!("process_lock\tfailed\t{error}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    if matches!(
        command.kind,
        command_args::CommandKind::PreFpga | command_args::CommandKind::ListOnly
    ) {
        dispatch_pre_fpga(&cmd, &args);
        return;
    }

    let _display_owner_lock = if command_args::requires_display_owner(&cmd) {
        match DisplayOwnerLock::acquire_default() {
            Ok(lock) => {
                crate::ui_logln!("display_owner_lock\tacquired\t{}", lock.path().display());
                Some(lock)
            }
            Err(error) => {
                crate::ui_errln!("display_owner_lock\trefused\t{error}");
                std::process::exit(13);
            }
        }
    } else {
        None
    };

    let mut f = match Fpga::open() {
        Ok(f) => f,
        Err(e) => {
            crate::ui_errln!("failed to open FPGA (/dev/mem): {e}");
            std::process::exit(1);
        }
    };

    dispatch_fpga(&cmd, &mut f);
}

enum ProcessLockState {
    Acquired(MagikProcessLock),
    Active { pid: u32 },
}

struct MagikProcessLock {
    path: PathBuf,
    pid: u32,
}

impl MagikProcessLock {
    fn acquire_default() -> Result<ProcessLockState, String> {
        let path = std::env::var("MISTER_MAGIK_PROCESS_LOCK")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_PROCESS_LOCK_PATH));
        Self::acquire(&path)
    }

    fn acquire(path: &Path) -> Result<ProcessLockState, String> {
        let pid = std::process::id();
        acquire_pid_lock(path, pid, process_is_mister_magik_fb).map(|state| match state {
            PidLockDecision::Acquired => ProcessLockState::Acquired(Self {
                path: path.to_path_buf(),
                pid,
            }),
            PidLockDecision::Active { pid } => ProcessLockState::Active { pid },
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for MagikProcessLock {
    fn drop(&mut self) {
        remove_pid_lock_if_owner(&self.path, self.pid);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PidLockDecision {
    Acquired,
    Active { pid: u32 },
}

fn acquire_pid_lock<F>(path: &Path, pid: u32, is_active: F) -> Result<PidLockDecision, String>
where
    F: Fn(u32) -> bool,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    match create_lock_file(path, pid) {
        Ok(()) => return Ok(PidLockDecision::Acquired),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("create {}: {e}", path.display())),
    }
    if let Some(active_pid) = read_lock_pid(path).filter(|locked_pid| is_active(*locked_pid)) {
        return Ok(PidLockDecision::Active { pid: active_pid });
    }
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove stale {}: {e}", path.display())),
    }
    match create_lock_file(path, pid) {
        Ok(()) => Ok(PidLockDecision::Acquired),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if let Some(active_pid) =
                read_lock_pid(path).filter(|locked_pid| is_active(*locked_pid))
            {
                Ok(PidLockDecision::Active { pid: active_pid })
            } else {
                Err(format!(
                    "lock appeared but owner is not active: {}",
                    path.display()
                ))
            }
        }
        Err(e) => Err(format!("create {}: {e}", path.display())),
    }
}

fn remove_pid_lock_if_owner(path: &Path, pid: u32) {
    let should_remove = read_lock_pid(path)
        .map(|locked_pid| locked_pid == pid)
        .unwrap_or(false);
    if should_remove {
        let _ = fs::remove_file(path);
    }
}

fn dispatch_pre_fpga(cmd: &str, args: &[String]) {
    match cmd {
        #[cfg(feature = "diagnostics")]
        "vsync-probe" => run_vsync_probe(),
        #[cfg(feature = "diagnostics")]
        "cpu-profile-smoke" => run_cpu_profile_smoke(),
        #[cfg(feature = "diagnostics")]
        "hidden-fb-copy-bench" => run_hidden_fb_copy_bench(),
        "library-refresh" => run_library_refresh(),
        "repair-catalog-projections" => run_repair_catalog_projections(),
        "request-library-rebuild" => run_request_library_rebuild(),
        "toggle-simple-joystick-setting" => run_toggle_simple_joystick_setting(),
        "reset-delete-database" => run_reset_delete_database(args),
        "reset-delete-screenshot-packs" => run_reset_delete_screenshot_packs(args),
        #[cfg(feature = "bench-tools")]
        "media-bench-download" => media_bench_download::run(),
        #[cfg(feature = "bench-tools")]
        "media-bench-save" => media_bench_save::run(),
        #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
        "preview-pack-bench" => preview_pack_bench::run(),
        #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
        "preview-index-refresh-bench" => run_preview_index_refresh_bench(),
        "library-sql" => run_library_sql(),
        #[cfg(feature = "diagnostics")]
        "hbmame-metadata-from-library" => run_hbmame_metadata_from_library(),
        #[cfg(feature = "bench-tools")]
        "launch-prep-bench" => launch_preparation::run_launch_prep_bench(),
        #[cfg(mister_experiments)]
        "experiment-capabilities" => print_experiment_capabilities(),
        #[cfg(mister_experiments)]
        "preview-transitions" => print_preview_transitions(),
        #[cfg(mister_experiments)]
        "camera-effects" => ui_runner::print_camera_effects(),
        #[cfg(mister_experiments)]
        "sprite-effects" => ui_runner::print_sprite_effects(),
        #[cfg(mister_experiments)]
        "text-effects" => ui_runner::print_text_effects(),
        #[cfg(mister_experiments)]
        "raster-effects" => ui_runner::print_raster_effects(),
        #[cfg(mister_experiments)]
        "transition-effects" => ui_runner::print_transition_effects(),
        other => unknown_command(other),
    }
}

fn dispatch_fpga(cmd: &str, f: &mut Fpga) {
    match cmd {
        "read" => read_mode(f),
        "early-black" => early_black_route(f),
        "ui" => ui_runner::run_ui(f),
        #[cfg(mister_bench_scenes)]
        "scenes" => ui_runner::print_scenes(),
        #[cfg(mister_experiments)]
        "effects" => ui_runner::print_effects(),
        #[cfg(mister_experiments)]
        "effect-bench" => ui_effect_bench::run_effect_bench(f),
        #[cfg(feature = "diagnostics")]
        "input" => run_input(),
        #[cfg(feature = "diagnostics")]
        "library-scan-bench" => library_db::run_scan_bench(),
        other => unknown_command(other),
    }
}

fn unknown_command(cmd: &str) -> ! {
    crate::ui_errln!(
        "unknown command '{cmd}' (use: {})",
        command_args::command_usage()
    );
    std::process::exit(2);
}

fn reject_direct_launch_arg(arg: &str) -> ! {
    crate::ui_errln!(
        "direct launch argument '{arg}' is unsupported; launch games through MiSTer_MagiK supervision"
    );
    std::process::exit(2);
}

#[cfg(mister_experiments)]
fn print_preview_transitions() {
    crate::ui_logln!(
        "{}",
        screenshot_transitions::PreviewTransitionEffect::labels()
    );
}

fn print_experiment_capabilities() {
    #[cfg(mister_experiments)]
    {
        crate::ui_logln!("experiments=1");
        crate::ui_logln!("commands=effects,camera-effects,sprite-effects,text-effects,raster-effects,transition-effects,effect-bench");
    }
    #[cfg(not(mister_experiments))]
    {
        crate::ui_logln!("experiments=0");
        crate::ui_logln!("commands=");
    }
}

fn run_library_refresh() {
    let parent_boot = std::env::var_os("MISTER_MAGIK_PARENT").is_some();
    let database_exists = usable_library_database_exists(&library_db::default_sqlite_path());
    let force_foreground = std::env::var_os("MISTER_MAGIK_FOREGROUND_LIBRARY_REFRESH").is_some();
    if should_defer_parent_boot_library_refresh(parent_boot, database_exists, force_foreground) {
        crate::ui_logln!("library_refresh\tdeferred\tmissing_database_parent_boot");
        return;
    }
    let lock_path = library_refresh_lock_path();
    let lock = match LibraryRefreshLock::acquire(&lock_path) {
        Ok(RefreshLockState::Acquired(lock)) => lock,
        Ok(RefreshLockState::Active { pid }) => {
            crate::ui_logln!("library_refresh\tskipped\tactive_pid={pid}");
            return;
        }
        Err(e) => {
            crate::ui_errln!("library_refresh\tfailed\tlock {e}");
            std::process::exit(1);
        }
    };
    let mut progress = |title: &str, detail: &str| {
        crate::ui_logln!("library_refresh\tprogress\t{title}\t{detail}");
    };
    match library_db::rebuild_default_sqlite_database(Some(&mut progress)) {
        Ok(summary) => {
            drop(lock);
            crate::ui_logln!(
                "library_refresh\tdone\tskipped={} bytes={} scan_us={} discover_us={} classify_us={} import_us={} discoveries={} normal_files={} containers={} entries={}",
                summary.skipped,
                summary.bytes,
                summary.scan_us,
                summary.discover_us,
                summary.classify_us,
                summary.import_us,
                summary.discoveries,
                summary.normal_files,
                summary.containers,
                summary.entries
            );
        }
        Err(e) => {
            drop(lock);
            crate::ui_errln!("library_refresh\tfailed\t{e}");
            std::process::exit(1);
        }
    }
}

fn run_repair_catalog_projections() {
    let started = std::time::Instant::now();
    match library_db::rewrite_default_catalog_projections(arcade_catalog::DEFAULT_ARCADE_ROOT) {
        Ok(summary) => crate::ui_logln!(
            "catalog_projection_repair_tsv\tstatus=ok\telapsed_us={}\tload_us={}\trepair_us={}\tgames={}\tsummary_bytes={}\tnavigation_bytes={}",
            started.elapsed().as_micros(),
            summary.load_us,
            summary.repair_us,
            summary.games,
            summary.summary_bytes,
            summary.navigation_bytes
        ),
        Err(e) => {
            crate::ui_errln!(
                "catalog_projection_repair_tsv\tstatus=failed\telapsed_us={}\terror={e}",
                started.elapsed().as_micros()
            );
            std::process::exit(1);
        }
    }
}

fn run_request_library_rebuild() {
    match launcher::request_library_rebuild_on_next_boot() {
        Ok(()) => crate::ui_logln!("request_library_rebuild\tdone"),
        Err(e) => {
            crate::ui_errln!("request_library_rebuild\tfailed\t{e}");
            std::process::exit(1);
        }
    }
}

fn run_toggle_simple_joystick_setting() {
    let mut settings = settings::MagikSettings::load();
    settings.simple_joystick_handling = !settings.simple_joystick_handling;
    match settings.save() {
        Ok(()) => crate::ui_logln!(
            "toggle_simple_joystick_setting\tdone\tsimple_joystick_handling={}",
            settings.simple_joystick_handling
        ),
        Err(e) => {
            crate::ui_errln!("toggle_simple_joystick_setting\tfailed\t{e}");
            std::process::exit(1);
        }
    }
}

fn run_reset_delete_database(args: &[String]) {
    if args
        .get(2)
        .is_some_and(|arg| arg == "-h" || arg == "--help")
    {
        crate::ui_logln!("usage: mister-magik-fb reset-delete-database");
        return;
    }
    match library_db::remove_default_sqlite_database() {
        Ok(()) => crate::ui_logln!("reset_delete_database\tdone"),
        Err(e) => {
            crate::ui_errln!("reset_delete_database\tfailed\t{e}");
            std::process::exit(1);
        }
    }
}

fn run_reset_delete_screenshot_packs(args: &[String]) {
    if args
        .get(2)
        .is_some_and(|arg| arg == "-h" || arg == "--help")
    {
        crate::ui_logln!("usage: mister-magik-fb reset-delete-screenshot-packs");
        return;
    }
    match launcher::delete_screenshot_packs() {
        Ok(removed) => crate::ui_logln!("reset_delete_screenshot_packs\tdone\tremoved={removed}"),
        Err(e) => {
            crate::ui_errln!("reset_delete_screenshot_packs\tfailed\t{e}");
            std::process::exit(1);
        }
    }
}

const DEFAULT_LIBRARY_REFRESH_LOCK_PATH: &str = "/tmp/mister-magik/library-refresh.lock";

fn usable_library_database_exists(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.len() > 0)
        .unwrap_or(false)
}

fn library_refresh_lock_path() -> PathBuf {
    std::env::var("MISTER_LIBRARY_REFRESH_LOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_LIBRARY_REFRESH_LOCK_PATH))
}

enum RefreshLockState {
    Acquired(LibraryRefreshLock),
    Active { pid: u32 },
}

struct LibraryRefreshLock {
    path: PathBuf,
    pid: u32,
}

impl LibraryRefreshLock {
    fn acquire(path: &Path) -> Result<RefreshLockState, String> {
        let pid = std::process::id();
        acquire_library_refresh_lock(path, pid, process_is_library_refresh).map(|state| match state
        {
            RefreshLockDecision::Acquired => RefreshLockState::Acquired(Self {
                path: path.to_path_buf(),
                pid,
            }),
            RefreshLockDecision::Active { pid } => RefreshLockState::Active { pid },
        })
    }
}

impl Drop for LibraryRefreshLock {
    fn drop(&mut self) {
        remove_pid_lock_if_owner(&self.path, self.pid);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshLockDecision {
    Acquired,
    Active { pid: u32 },
}

fn acquire_library_refresh_lock<F>(
    path: &Path,
    pid: u32,
    is_active_refresh: F,
) -> Result<RefreshLockDecision, String>
where
    F: Fn(u32) -> bool,
{
    acquire_pid_lock(path, pid, is_active_refresh).map(|decision| match decision {
        PidLockDecision::Acquired => RefreshLockDecision::Acquired,
        PidLockDecision::Active { pid } => RefreshLockDecision::Active { pid },
    })
}

fn create_lock_file(path: &Path, pid: u32) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    writeln!(file, "{pid}")?;
    Ok(())
}

fn read_lock_pid(path: &Path) -> Option<u32> {
    let mut text = String::new();
    File::open(path).ok()?.read_to_string(&mut text).ok()?;
    text.trim().parse::<u32>().ok()
}

fn process_is_library_refresh(pid: u32) -> bool {
    process_cmdline_parts(pid).is_some_and(|parts| {
        parts.iter().any(|part| part.ends_with("mister-magik-fb"))
            && parts.iter().any(|part| *part == "library-refresh")
    })
}

fn process_is_mister_magik_fb(pid: u32) -> bool {
    process_cmdline_parts(pid)
        .is_some_and(|parts| parts.iter().any(|part| part.ends_with("mister-magik-fb")))
}

fn process_cmdline_parts(pid: u32) -> Option<Vec<String>> {
    let path = PathBuf::from(format!("/proc/{pid}/cmdline"));
    let bytes = fs::read(path).ok()?;
    Some(
        bytes
            .split(|byte| *byte == 0)
            .filter_map(|part| std::str::from_utf8(part).ok())
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
    )
}

fn should_defer_parent_boot_library_refresh(
    parent_boot: bool,
    database_exists: bool,
    force_foreground: bool,
) -> bool {
    parent_boot && !database_exists && !force_foreground
}

fn run_library_sql() {
    let args: Vec<String> = std::env::args().skip(2).collect();
    match library_db::run_sqlite_inspect_cli(&args) {
        Ok(output) => {
            crate::ui_log!("{output}");
            if !output.ends_with('\n') {
                crate::ui_logln!();
            }
        }
        Err(e) => {
            crate::ui_errln!("library_sql\tfailed\t{e}");
            std::process::exit(1);
        }
    }
}

fn run_hbmame_metadata_from_library() {
    match library_db::write_default_hbmame_metadata_from_library() {
        Ok(summary) => {
            crate::ui_logln!(
                "hbmame_metadata_from_library\tdone\tpath={}\trows={}",
                summary.path.display(),
                summary.rows
            );
        }
        Err(e) => {
            crate::ui_errln!("hbmame_metadata_from_library\tfailed\t{e}");
            std::process::exit(1);
        }
    }
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
fn run_preview_index_refresh_bench() {
    let label = std::env::args()
        .nth(2)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "PREVIEW-INDEX-REFRESH".to_string());
    crate::ui_logln!("{}", library_db::PREVIEW_INDEX_REFRESH_TSV_HEADER);
    match library_db::refresh_default_preview_index_flags(&label) {
        Ok(rows) => {
            for row in rows {
                crate::ui_logln!("{}", row.to_tsv());
            }
        }
        Err(e) => {
            crate::ui_errln!("preview_index_refresh\tfailed\t{e}");
            std::process::exit(1);
        }
    }
}

fn run_cpu_profile_smoke() {
    let secs = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(3);
    if std::env::var("MISTER_PPROF").ok().as_deref() != Some("1") {
        crate::ui_errln!("cpu-profile-smoke requires MISTER_PPROF=1");
        std::process::exit(2);
    }
    crate::ui_logln!("cpu_profile_smoke: burning CPU for {secs}s");
    let cpu = cpu_profile::start();
    if cpu.is_none() {
        crate::ui_errln!("cpu_profile_smoke: profiler did not start");
        std::process::exit(1);
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let mut state = 0x1234_5678_9abc_def0_u64;
    let mut rounds = 0_u64;
    while std::time::Instant::now() < until {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1)
            .rotate_left((state & 31) as u32);
        std::hint::black_box(state);
        rounds = rounds.wrapping_add(1);
    }
    crate::ui_logln!("cpu_profile_smoke: rounds={rounds} state={state:#018x}");
    match cpu_profile::finish(cpu) {
        Ok(Some(summary)) if summary.sample_hits > 0 && summary.bytes > 0 => {
            crate::ui_logln!(
                "cpu_profile_smoke: ok samples={} stacks={} duration={:.1}s hz={} bytes={} out={}",
                summary.sample_hits,
                summary.sample_stacks,
                summary.duration_secs,
                summary.hz,
                summary.bytes,
                summary.out_path
            );
        }
        Ok(Some(summary)) => {
            crate::ui_errln!(
                "cpu_profile_smoke: profiler produced unusable output samples={} bytes={}",
                summary.sample_hits,
                summary.bytes
            );
            std::process::exit(1);
        }
        Ok(None) => {
            crate::ui_errln!("cpu_profile_smoke: profiling feature is not enabled");
            std::process::exit(1);
        }
        Err(e) => {
            crate::ui_errln!("{e}");
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "diagnostics")]
fn run_hidden_fb_copy_bench() {
    use slint::platform::software_renderer::Rgb565Pixel;

    let frames = std::env::var("MISTER_HIDDEN_FB_COPY_BENCH_FRAMES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| std::env::args().nth(2).and_then(|value| value.parse().ok()))
        .unwrap_or(240)
        .max(1);
    let width = std::env::var("MISTER_HIDDEN_FB_COPY_BENCH_W")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(960);
    let height = std::env::var("MISTER_HIDDEN_FB_COPY_BENCH_H")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(540);
    let stride_bytes = rgb565_stride_bytes(width);
    let bytes_per_copy = stride_bytes.saturating_mul(height);
    let mut source = vec![Rgb565Pixel(0); width.saturating_mul(height)];
    for (i, pixel) in source.iter_mut().enumerate() {
        let x = i % width.max(1);
        let y = i / width.max(1);
        pixel.0 = (((x as u16) & 0x1f) << 11)
            | (((y as u16) & 0x3f) << 5)
            | ((x as u16 ^ y as u16) & 0x1f);
    }

    let index1 = HiddenRgb565BufferIndex::new(1).expect("hidden buffer 1 index");
    let index2 = HiddenRgb565BufferIndex::new(2).expect("hidden buffer 2 index");
    let mut buffer1 = match HiddenRgb565Framebuffer::open(index1, width, height, stride_bytes) {
        Ok(buffer) => buffer,
        Err(e) => {
            crate::ui_errln!("hidden_fb_copy_bench\tfailed\tbuffer=1\terror={e}");
            std::process::exit(1);
        }
    };
    let mut buffer2 = match HiddenRgb565Framebuffer::open(index2, width, height, stride_bytes) {
        Ok(buffer) => buffer,
        Err(e) => {
            crate::ui_errln!("hidden_fb_copy_bench\tfailed\tbuffer=2\terror={e}");
            std::process::exit(1);
        }
    };

    crate::ui_logln!("hidden_fb_copy_bench_header\tframe\tbuffer\tbytes\twall_us\tcpu_us\tmb_s");
    let bench_start = std::time::Instant::now();
    let mut total_wall_us = 0u128;
    let mut total_cpu_us = 0u128;
    for frame in 0..frames {
        let buffer_index = if frame % 2 == 0 { 1 } else { 2 };
        let target = if buffer_index == 1 {
            &mut buffer1
        } else {
            &mut buffer2
        };
        let source_index = frame % source.len();
        source[source_index].0 ^= 0xffff;
        let cpu_start = thread_cpu_us();
        let copy_start = std::time::Instant::now();
        let copied_bytes = match target.copy_full_frame(&source, width) {
            Ok(bytes) => bytes,
            Err(e) => {
                crate::ui_errln!(
                    "hidden_fb_copy_bench\tfailed\tframe={frame}\tbuffer={buffer_index}\terror={e}"
                );
                std::process::exit(1);
            }
        };
        let wall_us = copy_start.elapsed().as_micros() as u64;
        let cpu_us = elapsed_thread_cpu_us(cpu_start);
        total_wall_us += wall_us as u128;
        total_cpu_us += cpu_us as u128;
        let mb_s = mb_per_second(copied_bytes, wall_us);
        crate::ui_logln!(
            "hidden_fb_copy_bench_tsv\t{frame}\t{buffer_index}\t{copied_bytes}\t{wall_us}\t{cpu_us}\t{mb_s:.2}"
        );
    }
    let elapsed_us = bench_start.elapsed().as_micros() as u128;
    let total_bytes = (bytes_per_copy as u128).saturating_mul(frames as u128);
    crate::ui_logln!(
        "hidden_fb_copy_bench_summary\tframes={frames}\twidth={width}\theight={height}\tstride_bytes={stride_bytes}\ttotal_bytes={total_bytes}\telapsed_us={elapsed_us}\tavg_wall_us={}\tavg_cpu_us={}\tavg_mb_s={:.2}",
        total_wall_us / frames as u128,
        total_cpu_us / frames as u128,
        mb_per_second(total_bytes.min(usize::MAX as u128) as usize, elapsed_us as u64)
    );
}

#[cfg(feature = "diagnostics")]
fn mb_per_second(bytes: usize, us: u64) -> f64 {
    if us == 0 {
        return 0.0;
    }
    (bytes as f64 / 1_048_576.0) / (us as f64 / 1_000_000.0)
}

#[cfg(all(feature = "diagnostics", target_os = "linux"))]
fn thread_cpu_us() -> Option<u64> {
    let mut ts = std::mem::MaybeUninit::<libc::timespec>::uninit();
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, ts.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let ts = unsafe { ts.assume_init() };
    Some((ts.tv_sec as u64) * 1_000_000 + (ts.tv_nsec as u64) / 1_000)
}

#[cfg(all(feature = "diagnostics", not(target_os = "linux")))]
fn thread_cpu_us() -> Option<u64> {
    None
}

#[cfg(feature = "diagnostics")]
fn elapsed_thread_cpu_us(start: Option<u64>) -> u64 {
    start
        .and_then(|start| thread_cpu_us().map(|end| end.saturating_sub(start)))
        .unwrap_or_default()
}

fn run_vsync_probe() {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let frames = args
        .first()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(300);
    let work_us = args.get(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let mode = args.get(2).map(|s| s.as_str()).unwrap_or("pacer");
    if mode == "direct" {
        run_direct_vsync_probe(frames, work_us);
        return;
    }
    let mut pacer = VsyncPacer::from_env();
    crate::ui_logln!(
        "frame\tsource\twait_us\tperiod_us\tinferred_hz\tmiss_streak\tloop_delta_us\tmessage"
    );
    let mut last_frame_at: Option<std::time::Instant> = None;
    for frame in 0..frames {
        let frame_at = std::time::Instant::now();
        let pace = pacer.wait();
        let loop_delta_us = last_frame_at
            .map(|prev| frame_at.saturating_duration_since(prev).as_micros() as u64)
            .unwrap_or(0);
        last_frame_at = Some(frame_at);
        crate::ui_logln!(
            "{frame}\t{}\t{}\t{}\t{:.2}\t{}\t{}\t{}",
            pace.source.label(),
            pace.wait_us,
            pace.period_us,
            1_000_000.0 / pace.period_us as f64,
            pace.miss_streak,
            loop_delta_us,
            pace.message.as_deref().unwrap_or("")
        );
        if work_us > 0 {
            std::thread::sleep(std::time::Duration::from_micros(work_us));
        }
    }
    crate::ui_logln!(
        "vsync_probe_summary mode=pacer frames={frames} work_us={work_us} hits={} timeouts={} fallback_frames={} errors={} max_miss_streak={} inferred_hz={:.2}",
        pacer.hits(),
        pacer.timeouts(),
        pacer.fallback_frames(),
        pacer.errors(),
        pacer.max_miss_streak(),
        1_000_000.0 / pacer.period_us() as f64
    );
    if pacer.errors() > 0 {
        std::process::exit(1);
    }
}

fn run_direct_vsync_probe(frames: u64, work_us: u64) {
    let disp = match MappedRgb565Framebuffer::open_current_rgb565() {
        Ok(d) => d,
        Err(e) => {
            crate::ui_errln!("vsync-probe direct: failed to open current RGB565 display: {e}");
            std::process::exit(1);
        }
    };
    crate::ui_logln!(
        "frame\tsource\twait_us\tperiod_us\tinferred_hz\tmiss_streak\tloop_delta_us\tmessage"
    );
    let mut hits = 0u64;
    let mut timeouts = 0u64;
    let mut errors = 0u64;
    let mut miss_streak = 0u32;
    let mut max_miss_streak = 0u32;
    let mut period_us = 16_667u64;
    let mut last_hit_at: Option<std::time::Instant> = None;
    let mut last_frame_at: Option<std::time::Instant> = None;
    for frame in 0..frames {
        let frame_at = std::time::Instant::now();
        let status = disp.wait_vsync_status();
        let loop_delta_us = last_frame_at
            .map(|prev| frame_at.saturating_duration_since(prev).as_micros() as u64)
            .unwrap_or(0);
        last_frame_at = Some(frame_at);
        let (source, wait_us, message) = match status {
            VsyncWaitStatus::Hit { wait_us, at } => {
                hits += 1;
                miss_streak = 0;
                if let Some(prev) = last_hit_at {
                    let observed = at.saturating_duration_since(prev).as_micros() as u64;
                    if (8_000..=25_000).contains(&observed) {
                        period_us = ((period_us * 7) + observed) / 8;
                    }
                }
                last_hit_at = Some(at);
                ("vsync", wait_us, String::new())
            }
            VsyncWaitStatus::Timeout { wait_us } => {
                timeouts += 1;
                miss_streak += 1;
                max_miss_streak = max_miss_streak.max(miss_streak);
                ("timeout", wait_us, String::new())
            }
            VsyncWaitStatus::Error { wait_us, message } => {
                errors += 1;
                miss_streak += 1;
                max_miss_streak = max_miss_streak.max(miss_streak);
                ("error", wait_us, message)
            }
        };
        crate::ui_logln!(
            "{frame}\t{source}\t{wait_us}\t{period_us}\t{:.2}\t{miss_streak}\t{loop_delta_us}\t{message}",
            1_000_000.0 / period_us as f64
        );
        if work_us > 0 {
            std::thread::sleep(std::time::Duration::from_micros(work_us));
        }
    }
    crate::ui_logln!(
        "vsync_probe_summary mode=direct frames={frames} work_us={work_us} hits={hits} timeouts={timeouts} fallback_frames=0 errors={errors} max_miss_streak={max_miss_streak} inferred_hz={:.2}",
        1_000_000.0 / period_us as f64
    );
    if errors > 0 {
        std::process::exit(1);
    }
}

fn exec_mister(args: &[String]) {
    crate::ui_logln!("core handoff → {MISTER_BIN} {}", args[1..].join(" "));
    let c_path = CString::new(MISTER_BIN).expect("CString");
    let c_args: Vec<CString> = std::iter::once(c_path.clone())
        .chain(
            args[1..]
                .iter()
                .map(|s| CString::new(s.as_str()).expect("CString")),
        )
        .collect();
    let ptrs: Vec<*const libc::c_char> = c_args
        .iter()
        .map(|s| s.as_ptr())
        .chain([std::ptr::null()])
        .collect();
    // SAFETY: c_path and every argv pointer are NUL-terminated CStrings kept
    // alive across the call, and ptrs is terminated by a null pointer. On
    // success execv does not return; on failure we only inspect errno.
    let err = unsafe { libc::execv(c_path.as_ptr(), ptrs.as_ptr()) };
    crate::ui_errln!("execv({MISTER_BIN}) failed: {err}");
    std::process::exit(1);
}

fn early_black_route(f: &mut Fpga) {
    let runtime_geometry = detect_runtime_display_geometry_for_plan(f, "early-black");
    let display_plan = UiDisplayPlan::from_runtime_or_mister_ini_file(runtime_geometry);
    crate::ui_logln!("{}", display_plan.log_line());
    if display_plan.fallback {
        boot_analytics::event("display_plan_fallback", display_plan.log_line());
    }
    if let Err(e) = MappedRgb565Framebuffer::write_mister_mode_rgb565(
        display_plan.fb_w,
        display_plan.fb_h,
        rgb565_stride_bytes(display_plan.fb_w),
    ) {
        crate::ui_errln!("early-black: failed to set framebuffer mode: {e}");
        std::process::exit(1);
    }

    let mut disp = match MappedRgb565Framebuffer::open_rgb565(display_plan.fb_w, display_plan.fb_h)
    {
        Ok(d) => d,
        Err(e) => {
            crate::ui_errln!("early-black: failed to open /dev/fb0: {e}");
            std::process::exit(1);
        }
    };

    disp.clear_black();
    boot_analytics::event(
        "early_black_route_frame_copied",
        format!(
            "format={} w={} h={}",
            production_label(),
            disp.width(),
            disp.height()
        ),
    );

    let route = LauncherFramebufferRoute::for_scan(
        display_plan.scan_w,
        display_plan.scan_h,
        display_plan.direct_video,
    );
    let flag = match f.enable_launcher_framebuffer_route(route, disp.width(), disp.height()) {
        Ok(flag) => flag,
        Err(e) => {
            crate::ui_errln!("early-black: failed to route framebuffer: {e}");
            std::process::exit(1);
        }
    };
    settle_boot_black_frame("early-black", &mut disp, f, route);
    let route_mode = route.mode();
    boot_analytics::event(
        "early_black_route_completed",
        format!(
            "format={} w={} h={} scan={}x{} support_flag={flag}",
            production_label(),
            disp.width(),
            disp.height(),
            route_mode.hact,
            route_mode.vact
        ),
    );
    crate::ui_logln!(
        "early-black: routed {} {}x{} -> {}x{} support_flag={flag}",
        production_label(),
        disp.width(),
        disp.height(),
        route_mode.hact,
        route_mode.vact
    );
}

fn read_mode(f: &mut Fpga) {
    crate::ui_logln!("\n=== UIO_GET_VRES (0x23) ===");
    let cmd = match f.cmd_capture(UIO_GET_VRES) {
        Ok(cmd) => cmd,
        Err(e) => {
            crate::ui_errln!("failed to issue UIO_GET_VRES: {e}");
            std::process::exit(1);
        }
    };
    print_word("  cmd", cmd);
    let mut vres = [(0u16, 0u16); 16];
    for w in vres.iter_mut() {
        *w = match f.spi_capture(0) {
            Ok(w) => w,
            Err(e) => {
                f.disable_io();
                crate::ui_errln!("failed to read UIO_GET_VRES word: {e}");
                std::process::exit(1);
            }
        };
    }
    f.disable_io();
    for (i, w) in vres.iter().enumerate() {
        print_word(&format!("  w{i:<2}"), *w);
    }
    let lo = |i: usize| vres[i].1 as u32;
    crate::ui_logln!(
        "  -> width={} height={}",
        lo(1) | (lo(2) << 16),
        lo(3) | (lo(4) << 16)
    );

    crate::ui_logln!("\n=== UIO_GET_FB_PAR (0x40) ===");
    let cmd = match f.cmd_capture(UIO_GET_FB_PAR) {
        Ok(cmd) => cmd,
        Err(e) => {
            crate::ui_errln!("failed to issue UIO_GET_FB_PAR: {e}");
            std::process::exit(1);
        }
    };
    print_word("  cmd(crc)", cmd);
    let mut fbp = [(0u16, 0u16); 6];
    for w in fbp.iter_mut() {
        *w = match f.spi_capture(0) {
            Ok(w) => w,
            Err(e) => {
                f.disable_io();
                crate::ui_errln!("failed to read UIO_GET_FB_PAR word: {e}");
                std::process::exit(1);
            }
        };
    }
    f.disable_io();
    for (i, w) in fbp.iter().enumerate() {
        print_word(&format!("  w{i:<2}"), *w);
    }
    crate::ui_logln!(
        "  -> arx={} ary={} fb_fmt=0x{:04x} fb_w={} fb_h={} fb_en={}",
        fbp[0].1,
        fbp[1].1,
        fbp[2].1,
        fbp[3].1,
        fbp[4].1,
        fbp[2].1 & 0x40 != 0
    );
}

fn print_word(label: &str, w: (u16, u16)) {
    crate::ui_logln!(
        "{label} hi=0x{:04x} ({:5})   lo=0x{:04x} ({:5})",
        w.0,
        w.0,
        w.1,
        w.1
    );
}

fn run_input() {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let sub = args.first().map(|s| s.as_str()).unwrap_or("log");
    match sub {
        "calibrate" => {
            let path = args.get(1).map(|s| s.as_str());
            if let Err(e) = input::calibrate(path) {
                crate::ui_errln!("input calibrate failed: {e}");
                std::process::exit(1);
            }
        }
        "log" => {
            let (path, secs) = parse_input_log_args(&args[1..]);
            if let Err(e) = input::log_js_events(path, secs) {
                crate::ui_errln!("input log failed: {e}");
                std::process::exit(1);
            }
        }
        "sniff" => {
            let (path, secs) = parse_input_log_args(&args[1..]);
            if let Err(e) = input::sniff(path, secs) {
                crate::ui_errln!("input sniff failed: {e}");
                std::process::exit(1);
            }
        }
        other => {
            crate::ui_errln!(
                "unknown input subcommand '{other}' \
                 (use: input log [path] [secs] | input sniff [path] [secs] | input calibrate [path])"
            );
            std::process::exit(2);
        }
    }
}

fn parse_input_log_args(args: &[String]) -> (Option<&str>, u64) {
    match args.len() {
        0 => (None, 120),
        1 => {
            if let Ok(secs) = args[0].parse::<u64>() {
                (None, secs)
            } else {
                (Some(args[0].as_str()), 30)
            }
        }
        _ => {
            if args[1].parse::<u64>().is_ok() {
                (Some(args[0].as_str()), args[1].parse().unwrap())
            } else {
                (None, args[0].parse().unwrap_or(30))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "mister-magik-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn parent_boot_missing_database_defers_library_refresh_to_launcher_ui() {
        assert!(should_defer_parent_boot_library_refresh(true, false, false));
        assert!(!should_defer_parent_boot_library_refresh(true, true, false));
        assert!(!should_defer_parent_boot_library_refresh(
            false, false, false
        ));
        assert!(!should_defer_parent_boot_library_refresh(true, false, true));
    }

    #[test]
    fn zero_byte_library_database_is_not_usable_for_parent_boot_deferral() {
        let root = unique_temp_path("zero-byte-library-db");
        fs::create_dir_all(&root).expect("create temp dir");
        let db = root.join("library.sqlite3");
        fs::write(&db, b"").expect("write empty db");

        assert!(!usable_library_database_exists(&db));
        fs::write(&db, b"not empty").expect("write nonempty db");
        assert!(usable_library_database_exists(&db));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn process_lock_acquires_and_cleans_up() {
        let lock_path = unique_temp_path("process-lock-acquire").join("process.lock");
        let state = MagikProcessLock::acquire(&lock_path).expect("acquire process lock");
        let ProcessLockState::Acquired(lock) = state else {
            panic!("expected acquired process lock");
        };
        assert_eq!(read_lock_pid(&lock_path), Some(std::process::id()));

        drop(lock);

        assert!(!lock_path.exists());
        let _ = fs::remove_dir_all(lock_path.parent().unwrap());
    }

    #[test]
    fn process_lock_skips_when_active_owner_exists() {
        let lock_path = unique_temp_path("process-lock-active").join("process.lock");
        fs::create_dir_all(lock_path.parent().unwrap()).expect("create lock dir");
        create_lock_file(&lock_path, 7777).expect("seed lock");

        let decision =
            acquire_pid_lock(&lock_path, 8888, |pid| pid == 7777).expect("check process lock");

        assert_eq!(decision, PidLockDecision::Active { pid: 7777 });
        assert_eq!(read_lock_pid(&lock_path), Some(7777));
        let _ = fs::remove_dir_all(lock_path.parent().unwrap());
    }

    #[test]
    fn process_lock_recovers_stale_owner() {
        let lock_path = unique_temp_path("process-lock-stale").join("process.lock");
        fs::create_dir_all(lock_path.parent().unwrap()).expect("create lock dir");
        create_lock_file(&lock_path, 7777).expect("seed stale lock");

        let decision =
            acquire_pid_lock(&lock_path, 8888, |_| false).expect("replace stale process lock");

        assert_eq!(decision, PidLockDecision::Acquired);
        assert_eq!(read_lock_pid(&lock_path), Some(8888));
        let _ = fs::remove_dir_all(lock_path.parent().unwrap());
    }

    #[test]
    fn library_refresh_lock_acquires_and_cleans_up() {
        let lock_path = unique_temp_path("refresh-lock-acquire").join("library-refresh.lock");
        let decision =
            acquire_library_refresh_lock(&lock_path, 1234, |_| false).expect("acquire lock");
        assert_eq!(decision, RefreshLockDecision::Acquired);
        assert_eq!(read_lock_pid(&lock_path), Some(1234));

        let guard = LibraryRefreshLock {
            path: lock_path.clone(),
            pid: 1234,
        };
        drop(guard);

        assert!(!lock_path.exists());
        let _ = fs::remove_dir_all(lock_path.parent().unwrap());
    }

    #[test]
    fn library_refresh_lock_skips_when_active_owner_exists() {
        let lock_path = unique_temp_path("refresh-lock-active").join("library-refresh.lock");
        fs::create_dir_all(lock_path.parent().unwrap()).expect("create lock dir");
        create_lock_file(&lock_path, 7777).expect("seed lock");

        let decision =
            acquire_library_refresh_lock(&lock_path, 8888, |pid| pid == 7777).expect("check lock");

        assert_eq!(decision, RefreshLockDecision::Active { pid: 7777 });
        assert_eq!(read_lock_pid(&lock_path), Some(7777));
        let _ = fs::remove_dir_all(lock_path.parent().unwrap());
    }

    #[test]
    fn library_refresh_lock_recovers_stale_lock() {
        let lock_path = unique_temp_path("refresh-lock-stale").join("library-refresh.lock");
        fs::create_dir_all(lock_path.parent().unwrap()).expect("create lock dir");
        create_lock_file(&lock_path, 7777).expect("seed stale lock");

        let decision =
            acquire_library_refresh_lock(&lock_path, 8888, |_| false).expect("replace stale lock");

        assert_eq!(decision, RefreshLockDecision::Acquired);
        assert_eq!(read_lock_pid(&lock_path), Some(8888));
        let _ = fs::remove_dir_all(lock_path.parent().unwrap());
    }

    #[test]
    fn library_refresh_lock_drop_keeps_another_owners_lock() {
        let lock_path = unique_temp_path("refresh-lock-drop-owner").join("library-refresh.lock");
        fs::create_dir_all(lock_path.parent().unwrap()).expect("create lock dir");
        create_lock_file(&lock_path, 7777).expect("seed other lock");

        let guard = LibraryRefreshLock {
            path: lock_path.clone(),
            pid: 8888,
        };
        drop(guard);

        assert_eq!(read_lock_pid(&lock_path), Some(7777));
        let _ = fs::remove_dir_all(lock_path.parent().unwrap());
    }
}
