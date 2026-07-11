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
//!     fb-map-report      report framebuffer ioctl metadata and mmap reach
//!     fb-map-bandwidth   compare fb0 and hidden-buffer write bandwidth
//!     plugin-map-report  report stock-kernel plugin probe metadata
//!     plugin-map-bandwidth
//!                        benchmark plugin probe mappings
//!     fpga-latch-report  report FPGA vblank-latched framebuffer capability
//!     fpga-latch-post-report
//!                        fill one plugin hidden slot and post it through FPGA latch
//!     fpga-latch-pattern
//!                        fill plugin hidden slots and vblank-latch them in FPGA
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
//!     framebuffer-stream-scalar-bench
//!                        measure the production RGB565 scalar decimator
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
#[cfg(feature = "diagnostics")]
use std::os::unix::io::AsRawFd;
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
mod spring_animation;
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

#[cfg(all(feature = "diagnostics", feature = "ui"))]
use fpga::LatchedFbufGeometry;
use fpga::{Fpga, UIO_GET_FB_PAR, UIO_GET_VRES};
#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
use fpga::{MAGIK_FBUF_LATCH_MAGIC, MAGIK_FBUF_STATUS_MAGIC};
use mister_magik_fb::framebuffer::format::{production_label, rgb565_stride_bytes};
#[cfg(feature = "diagnostics")]
use mister_magik_fb::framebuffer::hidden::{HiddenRgb565BufferIndex, HiddenRgb565Framebuffer};
use mister_magik_fb::framebuffer::mapped::MappedRgb565Framebuffer;
use mister_magik_fb::framebuffer::ownership::DisplayOwnerLock;
#[cfg(all(feature = "diagnostics", feature = "ui"))]
use mister_magik_fb::framebuffer::plugin_probe::PluginHiddenRgb565Framebuffer;
#[cfg(all(feature = "diagnostics", feature = "ui"))]
use mister_magik_fb::framebuffer::route::FramebufferRouteMode;
use mister_magik_fb::framebuffer::vsync::{VsyncPacer, VsyncWaitStatus};
use ui_display::{UiDisplay, UiDisplayPlan};
use ui_runner::launcher_display_session::LauncherDisplaySession;
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
        #[cfg(feature = "diagnostics")]
        "fb-map-report" => run_fb_map_report(),
        #[cfg(feature = "diagnostics")]
        "fb-map-bandwidth" => run_fb_map_bandwidth(),
        #[cfg(feature = "diagnostics")]
        "plugin-map-report" => run_plugin_map_report(),
        #[cfg(feature = "diagnostics")]
        "plugin-map-bandwidth" => run_plugin_map_bandwidth(),
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
        #[cfg(feature = "bench-tools")]
        "framebuffer-stream-scalar-bench" => {
            if !mister_magik_fb::framebuffer::downsample::run_scalar_bench() {
                std::process::exit(1);
            }
        }
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
        #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
        "fpga-latch-report" => run_fpga_latch_report(),
        #[cfg(all(feature = "diagnostics", feature = "ui"))]
        "fpga-latch-post-report" => run_fpga_latch_post_report(f),
        #[cfg(all(feature = "diagnostics", feature = "ui"))]
        "fpga-latch-pattern" => run_fpga_latch_pattern(f),
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
fn run_fb_map_report() {
    let width = std::env::var("MISTER_FB_MAP_REPORT_W")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(960);
    let height = std::env::var("MISTER_FB_MAP_REPORT_H")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(540);
    let stride_bytes = rgb565_stride_bytes(width);
    let frame_bytes = stride_bytes.saturating_mul(height);
    let double_bytes = frame_bytes.saturating_mul(2);

    let raw = match MappedRgb565Framebuffer::raw_diagnostics() {
        Ok(raw) => raw,
        Err(e) => {
            crate::ui_errln!("fb_map_report\tfailed\tstage=ioctl\terror={e}");
            std::process::exit(1);
        }
    };

    crate::ui_logln!(
        "fb_map_report_fix_tsv\tid={}\tsmem_start=0x{:x}\tsmem_len={}\tline_length={}\ttype={}\ttype_aux={}\tvisual={}\txpanstep={}\typanstep={}\tywrapstep={}\tmmio_start=0x{:x}\tmmio_len={}\taccel={}\tcapabilities=0x{:x}",
        raw.id,
        raw.smem_start,
        raw.smem_len,
        raw.line_length,
        raw.type_,
        raw.type_aux,
        raw.visual,
        raw.xpanstep,
        raw.ypanstep,
        raw.ywrapstep,
        raw.mmio_start,
        raw.mmio_len,
        raw.accel,
        raw.capabilities
    );
    crate::ui_logln!(
        "fb_map_report_var_tsv\txres={}\tyres={}\txres_virtual={}\tyres_virtual={}\txoffset={}\tyoffset={}\tbpp={}\tred={}:{}:{}\tgreen={}:{}:{}\tblue={}:{}:{}\ttransp={}:{}:{}\tvmode={}\trotate={}\tcolorspace={}",
        raw.xres,
        raw.yres,
        raw.xres_virtual,
        raw.yres_virtual,
        raw.xoffset,
        raw.yoffset,
        raw.bits_per_pixel,
        raw.red_offset,
        raw.red_length,
        raw.red_msb_right,
        raw.green_offset,
        raw.green_length,
        raw.green_msb_right,
        raw.blue_offset,
        raw.blue_length,
        raw.blue_msb_right,
        raw.transp_offset,
        raw.transp_length,
        raw.transp_msb_right,
        raw.vmode,
        raw.rotate,
        raw.colorspace
    );
    crate::ui_logln!(
        "fb_map_report_expected_tsv\twidth={width}\theight={height}\tstride_bytes={stride_bytes}\tframe_bytes={frame_bytes}\tdouble_bytes={double_bytes}\thidden_slot_bytes={}",
        mister_magik_fb::framebuffer::hidden::MISTER_FB_SLOT_BYTES
    );

    let full_reported = raw.smem_len;
    let probes = [
        ("active_frame", frame_bytes),
        ("two_rgb565_frames", double_bytes),
        ("reported_smem_len", full_reported),
    ];
    let mmap_probes = match MappedRgb565Framebuffer::probe_mmap_lengths(&probes) {
        Ok(probes) => probes,
        Err(e) => {
            crate::ui_errln!("fb_map_report\tfailed\tstage=mmap_probe\terror={e}");
            std::process::exit(1);
        }
    };
    for probe in mmap_probes {
        crate::ui_logln!(
            "fb_map_report_mmap_tsv\tlabel={}\trequested_len={}\tok={}\terror={}",
            probe.label,
            probe.requested_len,
            bool_tsv(probe.ok),
            probe.error.unwrap_or_default()
        );
    }
}

#[cfg(feature = "diagnostics")]
fn run_fb_map_bandwidth() {
    use slint::platform::software_renderer::Rgb565Pixel;

    let frames = std::env::var("MISTER_FB_MAP_BANDWIDTH_FRAMES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| std::env::args().nth(2).and_then(|value| value.parse().ok()))
        .unwrap_or(120)
        .max(1);
    let width = std::env::var("MISTER_FB_MAP_BANDWIDTH_W")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(960);
    let height = std::env::var("MISTER_FB_MAP_BANDWIDTH_H")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(540);
    let stride_bytes = rgb565_stride_bytes(width);
    let frame_bytes = stride_bytes.saturating_mul(height);
    let mut source = make_rgb565_bench_source(width, height);

    crate::ui_logln!(
        "fb_map_bandwidth_header\tcase\tframes\twidth\theight\tstride_bytes\tbytes_per_frame"
    );
    crate::ui_logln!(
        "fb_map_bandwidth_case_tsv\tcase=fb0-active\tframes={frames}\twidth={width}\theight={height}\tstride_bytes={stride_bytes}\tbytes_per_frame={frame_bytes}"
    );
    let raw = MappedRgb565Framebuffer::raw_diagnostics().ok();
    let source_bytes_len = frame_bytes.min(source.len() * std::mem::size_of::<Rgb565Pixel>());
    if raw
        .as_ref()
        .map(|raw| raw.smem_len >= frame_bytes)
        .unwrap_or(false)
    {
        match Fb0ByteRange::open(frame_bytes, 0, frame_bytes) {
            Ok(mut fb0_range) => {
                let result = run_copy_samples(frames, frame_bytes, &mut source, |src| {
                    let src_bytes = rgb565_as_bytes(src, source_bytes_len);
                    fb0_range.copy_from(src_bytes).map_err(|e| e.to_string())
                });
                print_bandwidth_result("fb0-active", &result);
            }
            Err(e) => print_bandwidth_error("fb0-active", &format!("open range: {e}")),
        }
    } else {
        match MappedRgb565Framebuffer::open_rgb565(width, height) {
            Ok(mut fb0) => {
                let result = run_copy_samples(frames, frame_bytes, &mut source, |src| {
                    fb0.present_rows_565(src, 0, height)
                        .map(|_| frame_bytes)
                        .map_err(|e| e.to_string())
                });
                print_bandwidth_result("fb0-active", &result);
            }
            Err(e) => print_bandwidth_error("fb0-active", &format!("open /dev/fb0: {e}")),
        }
    }

    if raw
        .as_ref()
        .map(|raw| raw.smem_len >= frame_bytes.saturating_mul(2))
        .unwrap_or(false)
    {
        crate::ui_logln!(
            "fb_map_bandwidth_case_tsv\tcase=fb0-second-frame-range\tframes={frames}\twidth={width}\theight={height}\tstride_bytes={stride_bytes}\tbytes_per_frame={frame_bytes}"
        );
        match Fb0ByteRange::open(frame_bytes.saturating_mul(2), frame_bytes, frame_bytes) {
            Ok(mut fb0_range) => {
                let result = run_copy_samples(frames, frame_bytes, &mut source, |src| {
                    let src_bytes = rgb565_as_bytes(src, source_bytes_len);
                    fb0_range.copy_from(src_bytes).map_err(|e| e.to_string())
                });
                print_bandwidth_result("fb0-second-frame-range", &result);
            }
            Err(e) => print_bandwidth_error("fb0-second-frame-range", &format!("open range: {e}")),
        }
    } else {
        let smem_len = raw.as_ref().map(|raw| raw.smem_len).unwrap_or_default();
        print_bandwidth_skip(
            "fb0-second-frame-range",
            &format!(
                "smem_len {smem_len} is smaller than required {}",
                frame_bytes.saturating_mul(2)
            ),
        );
    }

    crate::ui_logln!(
        "fb_map_bandwidth_case_tsv\tcase=hidden-dev-mem-buffer1\tframes={frames}\twidth={width}\theight={height}\tstride_bytes={stride_bytes}\tbytes_per_frame={frame_bytes}"
    );
    match HiddenRgb565BufferIndex::new(1)
        .map_err(|e| e.to_string())
        .and_then(|index| {
            HiddenRgb565Framebuffer::open(index, width, height, stride_bytes)
                .map_err(|e| e.to_string())
        }) {
        Ok(mut hidden) => {
            let result = run_copy_samples(frames, frame_bytes, &mut source, |src| {
                hidden
                    .copy_full_frame(src, width)
                    .map_err(|e| e.to_string())
            });
            print_bandwidth_result("hidden-dev-mem-buffer1", &result);
        }
        Err(e) => print_bandwidth_error("hidden-dev-mem-buffer1", &format!("open hidden: {e}")),
    }
}

#[cfg(feature = "diagnostics")]
const PLUGIN_PROBE_DEVICE: &str = "/dev/mister-magik-plugin-probe";

#[cfg(feature = "diagnostics")]
const PLUGIN_PROBE_REGION_OFFSET_BYTES: usize = 1024 * 1024;

#[cfg(feature = "diagnostics")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct PluginProbeRegion {
    index: usize,
    name: String,
    available: bool,
    phys: String,
    len: usize,
    dma_owned: bool,
}

#[cfg(feature = "diagnostics")]
fn run_plugin_map_report() {
    match MappedRgb565Framebuffer::raw_diagnostics() {
        Ok(raw) => {
            crate::ui_logln!(
                "plugin_map_fb0_tsv\tid={}\tsmem_start=0x{:x}\tsmem_len={}\tline_length={}\txres={}\tyres={}\txres_virtual={}\tyres_virtual={}\tbpp={}",
                raw.id,
                raw.smem_start,
                raw.smem_len,
                raw.line_length,
                raw.xres,
                raw.yres,
                raw.xres_virtual,
                raw.yres_virtual,
                raw.bits_per_pixel
            );
        }
        Err(e) => crate::ui_logln!("plugin_map_fb0_tsv\terror={e}"),
    }

    let metadata = match read_plugin_probe_metadata() {
        Ok(metadata) => metadata,
        Err(e) => {
            crate::ui_errln!("plugin_map_report\tfailed\tstage=read_probe\terror={e}");
            std::process::exit(1);
        }
    };
    for line in metadata.lines() {
        crate::ui_logln!("{line}");
    }

    let regions = parse_plugin_probe_regions(&metadata);
    if regions.is_empty() {
        crate::ui_errln!("plugin_map_report\tfailed\tstage=parse_regions\terror=no regions");
        std::process::exit(1);
    }
    for region in regions {
        let probe = PluginProbeByteRange::probe(region.index, region.len);
        crate::ui_logln!(
            "plugin_map_mmap_tsv\tindex={}\tname={}\tavailable={}\trequested_len={}\tok={}\terror={}",
            region.index,
            region.name,
            bool_tsv(region.available),
            region.len,
            bool_tsv(probe.is_ok()),
            probe.err().map(|e| e.to_string()).unwrap_or_default()
        );
    }
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
fn run_fpga_latch_report() {
    let mut fpga = match Fpga::open() {
        Ok(fpga) => fpga,
        Err(e) => {
            crate::ui_errln!("fpga_latch_report_failed\tstage=open_fpga\terror={e}");
            std::process::exit(1);
        }
    };

    let set_probe = match fpga.probe_magik_latched_fbuf_set() {
        Ok((hi, lo)) => (hi, lo, String::new()),
        Err(e) => (0, 0, e.to_string()),
    };
    let set_supported =
        set_probe.0 == MAGIK_FBUF_LATCH_MAGIC || set_probe.1 == MAGIK_FBUF_LATCH_MAGIC;
    crate::ui_logln!(
        "fpga_latch_set_probe_tsv\tcmd=0x{:02x}\tsupported={}\tmagic_expected=0x{:04x}\tack_high=0x{:04x}\tack_low=0x{:04x}\terror={}",
        fpga::MAGIK_UIO_SET_FBUF_LATCH,
        bool_tsv(set_supported),
        MAGIK_FBUF_LATCH_MAGIC,
        set_probe.0,
        set_probe.1,
        set_probe.2
    );

    let status = match fpga.read_magik_latched_fbuf_status() {
        Ok(status) => status,
        Err(e) => {
            crate::ui_logln!(
                "fpga_latch_status_tsv\tcmd=0x{:02x}\tsupported=0\tmagic_expected=0x{:04x}\terror={e}",
                fpga::MAGIK_UIO_GET_FBUF_LATCH,
                MAGIK_FBUF_STATUS_MAGIC
            );
            if set_supported {
                std::process::exit(1);
            }
            return;
        }
    };
    crate::ui_logln!(
        "fpga_latch_status_tsv\tcmd=0x{:02x}\tsupported={}\tmagic_expected=0x{:04x}\tack_high=0x{:04x}\tack_low=0x{:04x}\tactive_sequence={}\tpending_sequence={}\tpending={}\tpending_enabled={}\tactive_enabled={}\tflip_count={}\tpost_count={}\tdrop_count={}\tactive_base=0x{:08x}\tactive_width={}\tactive_height={}\tactive_stride={}",
        fpga::MAGIK_UIO_GET_FBUF_LATCH,
        bool_tsv(status.supported()),
        MAGIK_FBUF_STATUS_MAGIC,
        status.magic_hi,
        status.magic_lo,
        status.active_sequence,
        status.pending_sequence,
        bool_tsv(status.pending()),
        bool_tsv(status.pending_enabled()),
        bool_tsv(status.active_enabled()),
        status.flip_count,
        status.post_count,
        status.drop_count,
        status.active_base,
        status.active_width,
        status.active_height,
        status.active_stride
    );
    match fpga.read_magik_scanout_mailbox_status() {
        Ok(mailbox) => crate::ui_logln!(
            "fpga_scanout_mailbox_status_tsv\tcmd=0x{:02x}\tsupported={}\tack_high=0x{:04x}\tack_low=0x{:04x}\tcapabilities=0x{:04x}\tactive_sequence={}\tpending_sequence={}\tpending={}\tpost_slot={}\tapply_count={}\terror_count={}\tepoch=0x{:08x}",
            fpga::MAGIK_UIO_GET_SCANOUT_MAILBOX,
            bool_tsv(mailbox.supported()),
            mailbox.magic_hi,
            mailbox.magic_lo,
            mailbox.capabilities,
            mailbox.active_sequence,
            mailbox.pending_sequence,
            bool_tsv((mailbox.flags & 0x0004) != 0),
            mailbox.flags & 0x0003,
            mailbox.apply_count,
            mailbox.error_count,
            mailbox.epoch
        ),
        Err(e) => crate::ui_logln!(
            "fpga_scanout_mailbox_status_tsv\tcmd=0x{:02x}\tsupported=0\terror={e}",
            fpga::MAGIK_UIO_GET_SCANOUT_MAILBOX
        ),
    }
}

#[cfg(all(feature = "diagnostics", feature = "ui"))]
fn run_fpga_latch_post_report(fpga: &mut Fpga) {
    use slint::platform::software_renderer::Rgb565Pixel;

    let width = 960usize;
    let height = 540usize;
    let stride_bytes = rgb565_stride_bytes(width);
    let mut buffer = match PluginHiddenRgb565Framebuffer::open(
        HiddenRgb565BufferIndex::new(1).expect("hidden slot 1 index"),
        width,
        height,
        stride_bytes,
    ) {
        Ok(buffer) => buffer,
        Err(e) => {
            crate::ui_errln!("fpga_latch_post_report_failed\tstage=open_plugin_buffer\terror={e}");
            std::process::exit(1);
        }
    };
    let base_addr = match buffer.physical_addr() {
        Ok(base_addr) => base_addr,
        Err(e) => {
            crate::ui_errln!("fpga_latch_post_report_failed\tstage=physical_addr\terror={e}");
            std::process::exit(1);
        }
    };
    let mut source = vec![Rgb565Pixel(0); width * height];
    fill_hidden_latch_pattern(&mut source, width, height, 0, 1);

    let copy_start = std::time::Instant::now();
    if let Err(e) = buffer.copy_full_frame(&source, width) {
        crate::ui_errln!("fpga_latch_post_report_failed\tstage=copy\terror={e}");
        std::process::exit(1);
    }
    let copy_us = copy_start.elapsed().as_micros() as u64;

    let route = FramebufferRouteMode::framebuffer_sized(width as u16, height as u16);
    let latch_geometry = LatchedFbufGeometry::new(width as u16, route, 1);
    let post_start = std::time::Instant::now();
    let post = fpga.post_magik_latched_fbuf_rgb565(
        1,
        base_addr,
        width as u16,
        height as u16,
        latch_geometry,
    );
    let post_us = post_start.elapsed().as_micros() as u64;
    let (ack_high, ack_low) = match post {
        Ok(ack) => ack,
        Err(e) => {
            crate::ui_errln!(
                "fpga_latch_post_report_failed\tstage=post\tcopy_us={copy_us}\tpost_us={post_us}\terror={e}"
            );
            std::process::exit(1);
        }
    };
    let supported = ack_high == MAGIK_FBUF_LATCH_MAGIC || ack_low == MAGIK_FBUF_LATCH_MAGIC;
    std::thread::sleep(std::time::Duration::from_millis(20));
    let status_start = std::time::Instant::now();
    let status = match fpga.read_magik_latched_fbuf_status() {
        Ok(status) => status,
        Err(e) => {
            crate::ui_errln!(
                "fpga_latch_post_report_failed\tstage=status\tcopy_us={copy_us}\tpost_us={post_us}\terror={e}"
            );
            std::process::exit(1);
        }
    };
    let status_us = status_start.elapsed().as_micros() as u64;

    crate::ui_logln!(
        "fpga_latch_post_report_tsv\tsequence=1\tbuffer=1\tphys=0x{base_addr:08x}\twidth={width}\theight={height}\tstride_bytes={stride_bytes}\tcopy_us={copy_us}\tpost_us={post_us}\tstatus_us={status_us}\tset_supported={}\tack_high=0x{:04x}\tack_low=0x{:04x}\tstatus_supported={}\tactive_sequence={}\tpending_sequence={}\tpending={}\tactive_enabled={}\tflip_count={}\tpost_count={}\tdrop_count={}\tactive_base=0x{:08x}",
        bool_tsv(supported),
        ack_high,
        ack_low,
        bool_tsv(status.supported()),
        status.active_sequence,
        status.pending_sequence,
        bool_tsv(status.pending()),
        bool_tsv(status.active_enabled()),
        status.flip_count,
        status.post_count,
        status.drop_count,
        status.active_base
    );
    if !supported || !status.supported() {
        std::process::exit(1);
    }
}

#[cfg(all(feature = "diagnostics", feature = "ui"))]
fn run_fpga_latch_pattern(fpga: &mut Fpga) {
    use slint::platform::software_renderer::Rgb565Pixel;

    let frames = std::env::var("MISTER_FPGA_LATCH_PATTERN_FRAMES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| std::env::args().nth(2).and_then(|value| value.parse().ok()))
        .unwrap_or(180)
        .max(1);
    let width = 960usize;
    let height = 540usize;
    let stride_bytes = rgb565_stride_bytes(width);
    let mut buffer1 = match PluginHiddenRgb565Framebuffer::open(
        HiddenRgb565BufferIndex::new(1).expect("hidden slot 1 index"),
        width,
        height,
        stride_bytes,
    ) {
        Ok(buffer) => buffer,
        Err(e) => {
            crate::ui_errln!("fpga_latch_pattern_failed\tstage=open\tbuffer=1\terror={e}");
            std::process::exit(1);
        }
    };
    let mut buffer2 = match PluginHiddenRgb565Framebuffer::open(
        HiddenRgb565BufferIndex::new(2).expect("hidden slot 2 index"),
        width,
        height,
        stride_bytes,
    ) {
        Ok(buffer) => buffer,
        Err(e) => {
            crate::ui_errln!("fpga_latch_pattern_failed\tstage=open\tbuffer=2\terror={e}");
            std::process::exit(1);
        }
    };
    let base1 = match buffer1.physical_addr() {
        Ok(base) => base,
        Err(e) => {
            crate::ui_errln!("fpga_latch_pattern_failed\tstage=physical_addr\tbuffer=1\terror={e}");
            std::process::exit(1);
        }
    };
    let base2 = match buffer2.physical_addr() {
        Ok(base) => base,
        Err(e) => {
            crate::ui_errln!("fpga_latch_pattern_failed\tstage=physical_addr\tbuffer=2\terror={e}");
            std::process::exit(1);
        }
    };
    let route = FramebufferRouteMode::framebuffer_sized(width as u16, height as u16);
    let latch_geometry = LatchedFbufGeometry::new(width as u16, route, 1);
    let mut source = vec![Rgb565Pixel(0); width * height];
    let mut copy_samples = Vec::with_capacity(frames);
    let mut post_samples = Vec::with_capacity(frames);
    let mut status_samples = Vec::with_capacity(frames);
    let mut unsupported_posts = 0usize;
    let period = std::time::Duration::from_micros(16_667);
    let mut next_deadline = std::time::Instant::now();

    crate::ui_logln!(
        "fpga_latch_pattern_header_tsv\tframes={frames}\twidth={width}\theight={height}\tstride_bytes={stride_bytes}\tbytes_per_frame={}\tbuffer1_phys=0x{base1:08x}\tbuffer2_phys=0x{base2:08x}",
        stride_bytes * height
    );
    crate::ui_logln!(
        "fpga_latch_pattern_frame_tsv\tframe\tsequence\tbuffer\tcopy_us\tpost_us\tstatus_us\tset_supported\tactive_sequence\tpending_sequence\tpending\tflip_count\tpost_count\tdrop_count\tactive_base"
    );

    for frame in 0..frames {
        let buffer_index = if frame % 2 == 0 { 1 } else { 2 };
        let sequence = (frame as u16).wrapping_add(1).max(1);
        fill_hidden_latch_pattern(&mut source, width, height, frame, buffer_index);

        let copy_start = std::time::Instant::now();
        let copy_result = if buffer_index == 1 {
            buffer1.copy_full_frame(&source, width)
        } else {
            buffer2.copy_full_frame(&source, width)
        };
        let copy_us = copy_start.elapsed().as_micros() as u64;
        if let Err(e) = copy_result {
            crate::ui_errln!(
                "fpga_latch_pattern_failed\tstage=copy\tframe={frame}\tbuffer={buffer_index}\terror={e}"
            );
            std::process::exit(1);
        }

        let base_addr = if buffer_index == 1 { base1 } else { base2 };
        let post_start = std::time::Instant::now();
        let post = fpga.post_magik_latched_fbuf_rgb565(
            sequence,
            base_addr,
            width as u16,
            height as u16,
            latch_geometry,
        );
        let post_us = post_start.elapsed().as_micros() as u64;
        let set_supported = match post {
            Ok((ack_high, ack_low)) => {
                ack_high == MAGIK_FBUF_LATCH_MAGIC || ack_low == MAGIK_FBUF_LATCH_MAGIC
            }
            Err(e) => {
                crate::ui_errln!(
                    "fpga_latch_pattern_failed\tstage=post\tframe={frame}\tbuffer={buffer_index}\tcopy_us={copy_us}\tpost_us={post_us}\terror={e}"
                );
                std::process::exit(1);
            }
        };
        if !set_supported {
            unsupported_posts += 1;
        }

        next_deadline += period;
        let now = std::time::Instant::now();
        if next_deadline > now {
            std::thread::sleep(next_deadline - now);
        } else {
            next_deadline = now;
        }

        let status_start = std::time::Instant::now();
        let status = match fpga.read_magik_latched_fbuf_status() {
            Ok(status) => status,
            Err(e) => {
                crate::ui_errln!(
                    "fpga_latch_pattern_failed\tstage=status\tframe={frame}\tbuffer={buffer_index}\terror={e}"
                );
                std::process::exit(1);
            }
        };
        let status_us = status_start.elapsed().as_micros() as u64;
        copy_samples.push(copy_us);
        post_samples.push(post_us);
        status_samples.push(status_us);
        crate::ui_logln!(
            "fpga_latch_pattern_frame_tsv\t{frame}\t{sequence}\t{buffer_index}\t{copy_us}\t{post_us}\t{status_us}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t0x{:08x}",
            bool_tsv(set_supported),
            status.active_sequence,
            status.pending_sequence,
            bool_tsv(status.pending()),
            status.flip_count,
            status.post_count,
            status.drop_count,
            status.active_base
        );
    }

    let final_status = match fpga.read_magik_latched_fbuf_status() {
        Ok(status) => status,
        Err(e) => {
            crate::ui_errln!("fpga_latch_pattern_failed\tstage=final_status\terror={e}");
            std::process::exit(1);
        }
    };
    crate::ui_logln!(
        "fpga_latch_pattern_summary_tsv\tframes={}\tunsupported_posts={unsupported_posts}\tcopy_p50_us={}\tcopy_p95_us={}\tcopy_p99_us={}\tcopy_max_us={}\tpost_p50_us={}\tpost_p95_us={}\tpost_p99_us={}\tpost_max_us={}\tstatus_p50_us={}\tstatus_p95_us={}\tstatus_p99_us={}\tstatus_max_us={}\tfinal_active_sequence={}\tfinal_pending_sequence={}\tfinal_pending={}\tfinal_flip_count={}\tfinal_post_count={}\tfinal_drop_count={}",
        copy_samples.len(),
        percentile_u64(&copy_samples, 50),
        percentile_u64(&copy_samples, 95),
        percentile_u64(&copy_samples, 99),
        copy_samples.iter().copied().max().unwrap_or_default(),
        percentile_u64(&post_samples, 50),
        percentile_u64(&post_samples, 95),
        percentile_u64(&post_samples, 99),
        post_samples.iter().copied().max().unwrap_or_default(),
        percentile_u64(&status_samples, 50),
        percentile_u64(&status_samples, 95),
        percentile_u64(&status_samples, 99),
        status_samples.iter().copied().max().unwrap_or_default(),
        final_status.active_sequence,
        final_status.pending_sequence,
        bool_tsv(final_status.pending()),
        final_status.flip_count,
        final_status.post_count,
        final_status.drop_count
    );
    if unsupported_posts != 0 || !final_status.supported() {
        std::process::exit(1);
    }
}

#[cfg(feature = "diagnostics")]
fn run_plugin_map_bandwidth() {
    use slint::platform::software_renderer::Rgb565Pixel;

    let frames = std::env::var("MISTER_PLUGIN_MAP_BANDWIDTH_FRAMES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| std::env::args().nth(2).and_then(|value| value.parse().ok()))
        .unwrap_or(120)
        .max(1);
    let width = std::env::var("MISTER_PLUGIN_MAP_BANDWIDTH_W")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(960);
    let height = std::env::var("MISTER_PLUGIN_MAP_BANDWIDTH_H")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(540);
    let stride_bytes = rgb565_stride_bytes(width);
    let frame_bytes = stride_bytes.saturating_mul(height);
    let mut source = make_rgb565_bench_source(width, height);
    let source_bytes_len = frame_bytes.min(source.len() * std::mem::size_of::<Rgb565Pixel>());

    crate::ui_logln!(
        "plugin_map_bandwidth_header\tcase\tframes\twidth\theight\tstride_bytes\tbytes_per_frame"
    );

    crate::ui_logln!(
        "plugin_map_bandwidth_case_tsv\tcase=fb0-active\tframes={frames}\twidth={width}\theight={height}\tstride_bytes={stride_bytes}\tbytes_per_frame={frame_bytes}"
    );
    match Fb0ByteRange::open(frame_bytes, 0, frame_bytes) {
        Ok(mut fb0_range) => {
            let result = run_copy_samples(frames, frame_bytes, &mut source, |src| {
                let src_bytes = rgb565_as_bytes(src, source_bytes_len);
                fb0_range.copy_from(src_bytes).map_err(|e| e.to_string())
            });
            print_plugin_bandwidth_result("fb0-active", &result);
        }
        Err(e) => print_plugin_bandwidth_error("fb0-active", &format!("open range: {e}")),
    }

    let metadata = match read_plugin_probe_metadata() {
        Ok(metadata) => metadata,
        Err(e) => {
            print_plugin_bandwidth_error("plugin-probe", &format!("read probe metadata: {e}"));
            return;
        }
    };
    for region in parse_plugin_probe_regions(&metadata) {
        let case = format!("plugin-{}", region.name);
        crate::ui_logln!(
            "plugin_map_bandwidth_case_tsv\tcase={case}\tframes={frames}\twidth={width}\theight={height}\tstride_bytes={stride_bytes}\tbytes_per_frame={frame_bytes}\tindex={}\tphys={}\tdma_owned={}",
            region.index,
            region.phys,
            bool_tsv(region.dma_owned)
        );
        if !region.available {
            print_plugin_bandwidth_skip(&case, "region unavailable");
            continue;
        }
        if region.len < frame_bytes {
            print_plugin_bandwidth_skip(
                &case,
                &format!("region len {} is smaller than {frame_bytes}", region.len),
            );
            continue;
        }
        match PluginProbeByteRange::open(region.index, frame_bytes) {
            Ok(mut range) => {
                let result = run_copy_samples(frames, frame_bytes, &mut source, |src| {
                    let src_bytes = rgb565_as_bytes(src, source_bytes_len);
                    range.copy_from(src_bytes).map_err(|e| e.to_string())
                });
                print_plugin_bandwidth_result(&case, &result);
            }
            Err(e) => print_plugin_bandwidth_error(&case, &format!("open plugin range: {e}")),
        }
    }

    crate::ui_logln!(
        "plugin_map_bandwidth_case_tsv\tcase=hidden-dev-mem-buffer1\tframes={frames}\twidth={width}\theight={height}\tstride_bytes={stride_bytes}\tbytes_per_frame={frame_bytes}"
    );
    match HiddenRgb565BufferIndex::new(1)
        .map_err(|e| e.to_string())
        .and_then(|index| {
            HiddenRgb565Framebuffer::open(index, width, height, stride_bytes)
                .map_err(|e| e.to_string())
        }) {
        Ok(mut hidden) => {
            let result = run_copy_samples(frames, frame_bytes, &mut source, |src| {
                hidden
                    .copy_full_frame(src, width)
                    .map_err(|e| e.to_string())
            });
            print_plugin_bandwidth_result("hidden-dev-mem-buffer1", &result);
        }
        Err(e) => {
            print_plugin_bandwidth_error("hidden-dev-mem-buffer1", &format!("open hidden: {e}"))
        }
    }

    #[cfg(feature = "ui")]
    {
        crate::ui_logln!(
            "plugin_map_bandwidth_case_tsv\tcase=plugin-hidden-copy-full-frame-buffer1\tframes={frames}\twidth={width}\theight={height}\tstride_bytes={stride_bytes}\tbytes_per_frame={frame_bytes}"
        );
        match HiddenRgb565BufferIndex::new(1)
            .map_err(|e| e.to_string())
            .and_then(|index| {
                PluginHiddenRgb565Framebuffer::open(index, width, height, stride_bytes)
                    .map_err(|e| e.to_string())
            }) {
            Ok(mut hidden) => {
                let result = run_copy_samples(frames, frame_bytes, &mut source, |src| {
                    hidden
                        .copy_full_frame(src, width)
                        .map_err(|e| e.to_string())
                });
                print_plugin_bandwidth_result("plugin-hidden-copy-full-frame-buffer1", &result);
            }
            Err(e) => print_plugin_bandwidth_error(
                "plugin-hidden-copy-full-frame-buffer1",
                &format!("open plugin hidden: {e}"),
            ),
        }
    }
}

#[cfg(all(feature = "diagnostics", feature = "ui"))]
fn fill_hidden_latch_pattern(
    pixels: &mut [slint::platform::software_renderer::Rgb565Pixel],
    width: usize,
    height: usize,
    frame: usize,
    buffer_index: u8,
) {
    let bg = if buffer_index == 1 { 0x001f } else { 0xf800 };
    let fg = if buffer_index == 1 { 0xffe0 } else { 0x07ff };
    let phase = frame % 96;
    for y in 0..height {
        for x in 0..width {
            let stripe = ((x + phase) / 48) & 1;
            let band = ((y + frame / 2) / 36) & 1;
            let border = x < 8 || y < 8 || x >= width - 8 || y >= height - 8;
            let sequence_mark = y < 32 && x < ((frame % width).max(1));
            let color = if border || sequence_mark || (stripe ^ band) != 0 {
                fg
            } else {
                bg
            };
            pixels[y * width + x].0 = color;
        }
    }
}

#[cfg(feature = "diagnostics")]
fn read_plugin_probe_metadata() -> std::io::Result<String> {
    fs::read_to_string(PLUGIN_PROBE_DEVICE)
}

#[cfg(feature = "diagnostics")]
fn parse_plugin_probe_regions(metadata: &str) -> Vec<PluginProbeRegion> {
    metadata
        .lines()
        .filter_map(parse_plugin_probe_region)
        .collect()
}

#[cfg(feature = "diagnostics")]
fn parse_plugin_probe_region(line: &str) -> Option<PluginProbeRegion> {
    if !line.starts_with("plugin_probe_region_tsv\t") {
        return None;
    }
    let mut index = None;
    let mut name = None;
    let mut available = None;
    let mut phys = None;
    let mut len = None;
    let mut dma_owned = None;
    for field in line.split('\t').skip(1) {
        let (key, value) = field.split_once('=')?;
        match key {
            "index" => index = value.parse::<usize>().ok(),
            "name" => name = Some(value.to_string()),
            "available" => available = Some(value == "1"),
            "phys" => phys = Some(value.to_string()),
            "len" => len = value.parse::<usize>().ok(),
            "dma_owned" => dma_owned = Some(value == "1"),
            _ => {}
        }
    }
    Some(PluginProbeRegion {
        index: index?,
        name: name?,
        available: available?,
        phys: phys?,
        len: len?,
        dma_owned: dma_owned?,
    })
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
fn bool_tsv(value: bool) -> &'static str {
    if value {
        "1"
    } else {
        "0"
    }
}

#[cfg(feature = "diagnostics")]
fn make_rgb565_bench_source(
    width: usize,
    height: usize,
) -> Vec<slint::platform::software_renderer::Rgb565Pixel> {
    use slint::platform::software_renderer::Rgb565Pixel;
    let mut source = vec![Rgb565Pixel(0); width.saturating_mul(height)];
    for (i, pixel) in source.iter_mut().enumerate() {
        let x = i % width.max(1);
        let y = i / width.max(1);
        pixel.0 = (((x as u16) & 0x1f) << 11)
            | (((y as u16) & 0x3f) << 5)
            | ((x as u16 ^ y as u16) & 0x1f);
    }
    source
}

#[cfg(feature = "diagnostics")]
#[derive(Clone, Debug)]
struct CopySamples {
    frames: usize,
    bytes_per_frame: usize,
    total_bytes: usize,
    wall_us: Vec<u64>,
    cpu_us: Vec<u64>,
    error: Option<String>,
}

#[cfg(feature = "diagnostics")]
fn run_copy_samples<F>(
    frames: usize,
    bytes_per_frame: usize,
    source: &mut [slint::platform::software_renderer::Rgb565Pixel],
    mut copy: F,
) -> CopySamples
where
    F: FnMut(&[slint::platform::software_renderer::Rgb565Pixel]) -> Result<usize, String>,
{
    let mut wall_us = Vec::with_capacity(frames);
    let mut cpu_us = Vec::with_capacity(frames);
    let mut total_bytes = 0usize;
    for frame in 0..frames {
        if !source.is_empty() {
            let source_index = frame % source.len();
            source[source_index].0 ^= 0xffff;
        }
        let cpu_start = thread_cpu_us();
        let start = std::time::Instant::now();
        match copy(source) {
            Ok(bytes) => {
                wall_us.push(start.elapsed().as_micros() as u64);
                cpu_us.push(elapsed_thread_cpu_us(cpu_start));
                total_bytes = total_bytes.saturating_add(bytes);
            }
            Err(e) => {
                return CopySamples {
                    frames: wall_us.len(),
                    bytes_per_frame,
                    total_bytes,
                    wall_us,
                    cpu_us,
                    error: Some(format!("frame={frame} {e}")),
                };
            }
        }
    }
    CopySamples {
        frames,
        bytes_per_frame,
        total_bytes,
        wall_us,
        cpu_us,
        error: None,
    }
}

#[cfg(feature = "diagnostics")]
fn print_bandwidth_result(case: &str, samples: &CopySamples) {
    if let Some(error) = &samples.error {
        print_bandwidth_error(case, error);
        return;
    }
    crate::ui_logln!(
        "fb_map_bandwidth_summary_tsv\tcase={case}\tvalid=1\tframes={}\tbytes_per_frame={}\ttotal_bytes={}\tavg_wall_us={}\tp50_wall_us={}\tp95_wall_us={}\tp99_wall_us={}\tmax_wall_us={}\tavg_cpu_us={}\tp50_cpu_us={}\tp95_cpu_us={}\tp99_cpu_us={}\tmax_cpu_us={}\tavg_mb_s={:.2}\terror=",
        samples.frames,
        samples.bytes_per_frame,
        samples.total_bytes,
        avg_u64(&samples.wall_us),
        percentile_u64(&samples.wall_us, 50),
        percentile_u64(&samples.wall_us, 95),
        percentile_u64(&samples.wall_us, 99),
        samples.wall_us.iter().copied().max().unwrap_or_default(),
        avg_u64(&samples.cpu_us),
        percentile_u64(&samples.cpu_us, 50),
        percentile_u64(&samples.cpu_us, 95),
        percentile_u64(&samples.cpu_us, 99),
        samples.cpu_us.iter().copied().max().unwrap_or_default(),
        mb_per_second(samples.total_bytes, samples.wall_us.iter().copied().sum())
    );
}

#[cfg(feature = "diagnostics")]
fn print_bandwidth_error(case: &str, error: &str) {
    crate::ui_logln!(
        "fb_map_bandwidth_summary_tsv\tcase={case}\tvalid=0\tframes=0\tbytes_per_frame=0\ttotal_bytes=0\tavg_wall_us=0\tp50_wall_us=0\tp95_wall_us=0\tp99_wall_us=0\tmax_wall_us=0\tavg_cpu_us=0\tp50_cpu_us=0\tp95_cpu_us=0\tp99_cpu_us=0\tmax_cpu_us=0\tavg_mb_s=0.00\terror={error}"
    );
}

#[cfg(feature = "diagnostics")]
fn print_bandwidth_skip(case: &str, reason: &str) {
    crate::ui_logln!("fb_map_bandwidth_skip_tsv\tcase={case}\treason={reason}");
}

#[cfg(feature = "diagnostics")]
fn avg_u64(values: &[u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.iter().sum::<u64>() / values.len() as u64
}

#[cfg(feature = "diagnostics")]
fn percentile_u64(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut values = values.to_vec();
    values.sort_unstable();
    let index = values.len().saturating_mul(percentile).saturating_add(99) / 100;
    values[index.saturating_sub(1).min(values.len() - 1)]
}

#[cfg(feature = "diagnostics")]
fn rgb565_as_bytes(
    pixels: &[slint::platform::software_renderer::Rgb565Pixel],
    len: usize,
) -> &[u8] {
    let byte_len = len.min(std::mem::size_of_val(pixels));
    // SAFETY: Rgb565Pixel is layout-compatible with u16 in framebuffer::mapped
    // compile-time assertions, and the returned slice is read-only.
    unsafe { std::slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), byte_len) }
}

#[cfg(feature = "diagnostics")]
struct Fb0ByteRange {
    mem: *mut u8,
    map_len: usize,
    offset: usize,
    len: usize,
    _fb0: File,
}

#[cfg(feature = "diagnostics")]
impl Fb0ByteRange {
    fn open(map_len: usize, offset: usize, len: usize) -> std::io::Result<Self> {
        if len == 0
            || offset
                .checked_add(len)
                .map(|end| end > map_len)
                .unwrap_or(true)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid fb0 byte range offset={offset} len={len} map_len={map_len}"),
            ));
        }
        let fb0 = OpenOptions::new().read(true).write(true).open("/dev/fb0")?;
        // SAFETY: fd refers to /dev/fb0; mapping length is validated above and unmapped in Drop.
        let mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fb0.as_raw_fd(),
                0,
            )
        };
        if mem == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        if mem.is_null() {
            // SAFETY: mem/map_len were just returned by mmap.
            unsafe {
                libc::munmap(mem, map_len);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "fb0 range mmap returned null",
            ));
        }
        Ok(Self {
            mem: mem.cast::<u8>(),
            map_len,
            offset,
            len,
            _fb0: fb0,
        })
    }

    fn copy_from(&mut self, src: &[u8]) -> std::io::Result<usize> {
        if src.len() < self.len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("source has {} bytes, need {}", src.len(), self.len),
            ));
        }
        // SAFETY: offset/len were validated against map_len in open; &mut self prevents aliasing.
        let dst = unsafe { std::slice::from_raw_parts_mut(self.mem.add(self.offset), self.len) };
        dst.copy_from_slice(&src[..self.len]);
        Ok(self.len)
    }
}

#[cfg(feature = "diagnostics")]
impl Drop for Fb0ByteRange {
    fn drop(&mut self) {
        // SAFETY: mem/map_len come from successful mmap and are unmapped once here.
        unsafe {
            libc::munmap(self.mem.cast::<libc::c_void>(), self.map_len);
        }
    }
}

#[cfg(feature = "diagnostics")]
struct PluginProbeByteRange {
    mem: *mut u8,
    len: usize,
    _device: File,
}

#[cfg(feature = "diagnostics")]
impl PluginProbeByteRange {
    fn probe(index: usize, len: usize) -> std::io::Result<()> {
        Self::open(index, len).map(|_| ())
    }

    fn open(index: usize, len: usize) -> std::io::Result<Self> {
        if len == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "zero-length plugin range",
            ));
        }
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(PLUGIN_PROBE_DEVICE)?;
        let offset = index
            .checked_mul(PLUGIN_PROBE_REGION_OFFSET_BYTES)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "plugin offset overflow")
            })?;
        // SAFETY: fd refers to the plugin probe misc device; mapping length is
        // requested by diagnostics and unmapped in Drop.
        let mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                device.as_raw_fd(),
                offset as libc::off_t,
            )
        };
        if mem == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        if mem.is_null() {
            // SAFETY: mem/len were just returned by mmap.
            unsafe {
                libc::munmap(mem, len);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "plugin range mmap returned null",
            ));
        }
        Ok(Self {
            mem: mem.cast::<u8>(),
            len,
            _device: device,
        })
    }

    fn copy_from(&mut self, src: &[u8]) -> std::io::Result<usize> {
        if src.len() < self.len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("source has {} bytes, need {}", src.len(), self.len),
            ));
        }
        // SAFETY: mem/len come from successful mmap; &mut self prevents aliasing.
        let dst = unsafe { std::slice::from_raw_parts_mut(self.mem, self.len) };
        dst.copy_from_slice(&src[..self.len]);
        Ok(self.len)
    }
}

#[cfg(feature = "diagnostics")]
impl Drop for PluginProbeByteRange {
    fn drop(&mut self) {
        // SAFETY: mem/len come from successful mmap and are unmapped once here.
        unsafe {
            libc::munmap(self.mem.cast::<libc::c_void>(), self.len);
        }
    }
}

#[cfg(feature = "diagnostics")]
fn print_plugin_bandwidth_result(case: &str, samples: &CopySamples) {
    if let Some(error) = &samples.error {
        print_plugin_bandwidth_error(case, error);
        return;
    }
    crate::ui_logln!(
        "plugin_map_bandwidth_summary_tsv\tcase={case}\tvalid=1\tframes={}\tbytes_per_frame={}\ttotal_bytes={}\tavg_wall_us={}\tp50_wall_us={}\tp95_wall_us={}\tp99_wall_us={}\tmax_wall_us={}\tavg_cpu_us={}\tp50_cpu_us={}\tp95_cpu_us={}\tp99_cpu_us={}\tmax_cpu_us={}\tavg_mb_s={:.2}\terror=",
        samples.frames,
        samples.bytes_per_frame,
        samples.total_bytes,
        avg_u64(&samples.wall_us),
        percentile_u64(&samples.wall_us, 50),
        percentile_u64(&samples.wall_us, 95),
        percentile_u64(&samples.wall_us, 99),
        samples.wall_us.iter().copied().max().unwrap_or_default(),
        avg_u64(&samples.cpu_us),
        percentile_u64(&samples.cpu_us, 50),
        percentile_u64(&samples.cpu_us, 95),
        percentile_u64(&samples.cpu_us, 99),
        samples.cpu_us.iter().copied().max().unwrap_or_default(),
        mb_per_second(samples.total_bytes, samples.wall_us.iter().copied().sum())
    );
}

#[cfg(feature = "diagnostics")]
fn print_plugin_bandwidth_error(case: &str, error: &str) {
    crate::ui_logln!(
        "plugin_map_bandwidth_summary_tsv\tcase={case}\tvalid=0\tframes=0\tbytes_per_frame=0\ttotal_bytes=0\tavg_wall_us=0\tp50_wall_us=0\tp95_wall_us=0\tp99_wall_us=0\tmax_wall_us=0\tavg_cpu_us=0\tp50_cpu_us=0\tp95_cpu_us=0\tp99_cpu_us=0\tmax_cpu_us=0\tavg_mb_s=0.00\terror={error}"
    );
}

#[cfg(feature = "diagnostics")]
fn print_plugin_bandwidth_skip(case: &str, reason: &str) {
    crate::ui_logln!("plugin_map_bandwidth_skip_tsv\tcase={case}\treason={reason}");
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

    let ui = UiDisplay::for_plan(display_plan);
    let mut display_session = LauncherDisplaySession::new(&ui);
    let route = display_session.route();
    let flag = match display_session.enable_initial(f) {
        Ok(flag) => flag,
        Err(e) => {
            crate::ui_errln!("early-black: failed to route framebuffer: {e}");
            std::process::exit(1);
        }
    };
    settle_boot_black_frame("early-black", &mut disp, f, &mut display_session);
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
    #[cfg(feature = "diagnostics")]
    fn plugin_probe_region_parser_reads_module_metadata() {
        let metadata = "\
plugin_probe_header_tsv\tname=mister-magik-plugin-probe\tversion=2\tuts_release=5.15.1-MiSTer\topen_count=1\tmmap_count=0\tpage_size=4096\tregion_offset_pages=256\tregion_offset_bytes=1048576\tcache_mode=writecombine\n\
plugin_probe_region_tsv\tindex=0\tname=adjacent-fb-resource\tavailable=1\tphys=0x220fd200\tlen=1036800\tdma_owned=0\n\
plugin_probe_region_tsv\tindex=1\tname=hidden-slot-1\tavailable=1\tphys=0x22800000\tlen=1036800\tdma_owned=0\n\
plugin_probe_region_tsv\tindex=3\tname=plugin-owned-dma\tavailable=0\tphys=0x00000000\tlen=1036800\tdma_owned=1\n";

        let regions = parse_plugin_probe_regions(metadata);

        assert_eq!(
            regions,
            vec![
                PluginProbeRegion {
                    index: 0,
                    name: "adjacent-fb-resource".to_string(),
                    available: true,
                    phys: "0x220fd200".to_string(),
                    len: 1_036_800,
                    dma_owned: false,
                },
                PluginProbeRegion {
                    index: 1,
                    name: "hidden-slot-1".to_string(),
                    available: true,
                    phys: "0x22800000".to_string(),
                    len: 1_036_800,
                    dma_owned: false,
                },
                PluginProbeRegion {
                    index: 3,
                    name: "plugin-owned-dma".to_string(),
                    available: false,
                    phys: "0x00000000".to_string(),
                    len: 1_036_800,
                    dma_owned: true,
                },
            ]
        );
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
