// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

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
//!     purge-library-data --confirm
//!                        delete catalog and screenshot artifacts without rebooting
//!     reset-delete-screenshot-packs
//!                        delete screenshot media artifacts for fault tests
//!   Diagnostics:
//!     read               print live video mode + fb params
//!     vsync-probe        print per-frame vsync/fallback pacing diagnostics
//!     cpu-profile-smoke  burn CPU and verify profiler SVG output
//!     fb-map-report      report framebuffer ioctl metadata and mmap reach
//!     fb-map-bandwidth   compare supported framebuffer write paths
//!     scanout-slots-map-report  report stock-kernel scanout slots metadata
//!     fpga-latch-report  report FPGA vblank-latched framebuffer capability
//!     fpga-latch-post-report
//!                        fill one scanout slot and post it through FPGA latch
//!     fpga-latch-pattern
//!                        fill scanout slots and vblank-latch them in FPGA
//!     catalog-v3-inspect validate the registry, shards, state, and scanner cache
//!     catalog-v3-registry-report list system counts without opening system shards
//!     search-bench       benchmark persisted Arcade FTS5 search
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
//! apps/mister/BUILD.md for toolchain details.

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(feature = "diagnostics")]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static PROCESS_START_MONOTONIC_US: OnceLock<u64> = OnceLock::new();

fn device_monotonic_us() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid writable timespec and CLOCK_MONOTONIC has no
    // externally visible side effects.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return 0;
    }
    u64::try_from(ts.tv_sec)
        .unwrap_or(0)
        .saturating_mul(1_000_000)
        .saturating_add(u64::try_from(ts.tv_nsec).unwrap_or(0) / 1_000)
}

pub(crate) fn process_start_monotonic_us() -> u64 {
    *PROCESS_START_MONOTONIC_US.get_or_init(device_monotonic_us)
}

pub use mister_magik_fb::build_identity;
use mister_magik_mister_runtime::boot_analytics;
use mister_magik_mister_runtime::fpga;
use mister_magik_mister_runtime::settings;

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
use mister_magik_fb::preview_pack_bench;
#[cfg(mister_experiments)]
use mister_magik_fb::screenshot_transitions;
#[cfg(mister_experiments)]
use mister_magik_fb::ui_effect_bench;
pub use mister_magik_fb::{
    arcade_button_overrides, arcade_catalog, command_args, controller_db, framebuffer, input_event,
    input_repeat, input_state, launch_preparation, launcher, launcher_presentation,
    launcher_taxonomy, library_db, licenses, media_update, particle_engine, preview_worker,
    return_catalog_capsule, setup_nav, spring_animation, ui_errln, ui_log, ui_logln,
};
use mister_magik_fb::{
    cpu_profile, input, input_integrity_driver, pmu_probe, pmu_profile, search_bench, ui_display,
    ui_runner,
};
#[cfg(feature = "bench-tools")]
use mister_magik_fb::{media_bench_download, media_bench_save};

#[cfg(all(feature = "diagnostics", feature = "ui"))]
use fpga::LatchedFbufGeometry;
use fpga::{Fpga, MAGIK_FBUF_LATCH_MAGIC, MAGIK_FBUF_STATUS_MAGIC, UIO_GET_FB_PAR, UIO_GET_VRES};
use mister_magik_fb::framebuffer::format::{production_label, rgb565_stride_bytes};
use mister_magik_fb::framebuffer::mapped::MappedRgb565Framebuffer;
use mister_magik_fb::framebuffer::ownership::DisplayOwnerLock;
#[cfg(all(feature = "diagnostics", feature = "ui"))]
use mister_magik_fb::framebuffer::route::FramebufferRouteMode;
#[cfg(all(feature = "diagnostics", feature = "ui"))]
use mister_magik_fb::framebuffer::scanout_slots::{
    HiddenRgb565BufferIndex, SCANOUT_SLOTS_DEVICE, ScanoutSlotsRgb565Framebuffer,
    read_scanout_slots_layout,
};
use mister_magik_fb::framebuffer::vsync::{VsyncPacer, VsyncWaitStatus};
use ui_display::{UiDisplay, UiDisplayPlan};
use ui_runner::launcher_display_session::LauncherDisplaySession;
use ui_runner::ui_boot::{detect_runtime_display_geometry_for_plan, settle_boot_black_frame};

const DEFAULT_PROCESS_LOCK_PATH: &str = "/tmp/mister-magik/process.lock";
pub fn run() {
    let _ = process_start_monotonic_us();
    let process_entry_cpu_profile = cpu_profile::start_process_entry();
    let args: Vec<String> = std::env::args().collect();
    mister_magik_fb::crash_report::install_panic_hook(args.clone());
    let build_identity = build_identity::BuildIdentity::current();
    boot_analytics::event(
        "process_start",
        format!("args={} {}", args.join(" "), build_identity.log_detail()),
    );

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
    let process_config = mister_magik_fb::process_config::ProcessConfig::capture(&args, &cmd);
    let fault_config = process_config.fault().cloned();
    if let Err(error) =
        mister_magik_mister_runtime::direct_reset_fault::install_process_fault_config(
            fault_config.clone(),
        )
    {
        crate::ui_errln!("fault configuration initialization failed: {error}");
        std::process::exit(1);
    }

    let latch_readiness_json = process_config.diagnostics().latch_readiness_json;
    if cmd != command_args::CATALOG_INSPECT_COMMAND && !latch_readiness_json {
        crate::ui_logln!("mister-magik-fb [{cmd}] ({})", build_identity.log_detail());
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
        dispatch_pre_fpga(&cmd, &args, fault_config.as_ref());
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

    dispatch_fpga(
        &cmd,
        &mut f,
        process_entry_cpu_profile,
        fault_config.as_ref(),
        &process_config,
    );
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

fn dispatch_pre_fpga(
    cmd: &str,
    args: &[String],
    _fault_config: Option<&mister_magik_catalog::fs_fault::FaultConfig>,
) {
    match cmd {
        #[cfg(feature = "diagnostics")]
        "vsync-probe" => run_vsync_probe(),
        #[cfg(feature = "diagnostics")]
        "cpu-profile-smoke" => run_cpu_profile_smoke(),
        #[cfg(feature = "diagnostics")]
        "fb-map-report" => run_fb_map_report(),
        #[cfg(feature = "diagnostics")]
        "fb-map-bandwidth" => run_fb_map_bandwidth(),
        #[cfg(feature = "diagnostics")]
        "scanout-slots-map-report" => run_scanout_slots_map_report(),
        "library-refresh" => run_library_refresh(),
        "request-library-rebuild" => run_request_library_rebuild(),
        "toggle-simple-joystick-setting" => run_toggle_simple_joystick_setting(),
        "display-persist" => run_display_persist(args),
        "purge-library-data" => run_purge_library_data(args),
        "reset-delete-screenshot-packs" => run_reset_delete_screenshot_packs(args),
        "benchmark-capabilities" => print_benchmark_capabilities(),
        "input-integrity-driver" => input_integrity_driver::run(args.get(2..).unwrap_or_default()),
        "pmu-probe" => pmu_probe::run(),
        "pmu-profile" => pmu_profile::run(args.get(2..).unwrap_or_default()),
        "search-bench" => search_bench::run(),
        #[cfg(feature = "bench-tools")]
        "media-bench-download" => media_bench_download::run(),
        #[cfg(feature = "bench-tools")]
        "media-bench-save" => media_bench_save::run(),
        #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
        "preview-pack-bench" => preview_pack_bench::run(),
        #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
        "preview-index-refresh-bench" => run_preview_index_refresh_bench(),
        command_args::CATALOG_INSPECT_COMMAND => run_catalog_v3_inspect(),
        command_args::CATALOG_REGISTRY_REPORT_COMMAND => run_catalog_v3_registry_report(),
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

fn print_benchmark_capabilities() {
    crate::ui_logln!("{}", benchmark_capabilities());
}

fn benchmark_capabilities() -> serde_json::Value {
    let mut capabilities = serde_json::json!({
        "schema": "mister-magik-benchmark-capabilities-v1",
        "screensaver-pprof-v1": cfg!(feature = "profile"),
        "cold-boot-pprof-v1": cfg!(feature = "profile"),
        "particle-capacity-v1": true,
        "orientation-transition-v2": true,
        "orientation-transition-pprof-v1": cfg!(feature = "profile"),
        "settings-navigation-transition-v4": true,
        "settings-navigation-transition-pprof-v4": cfg!(feature = "profile"),
        "launcher-response-pprof-v1": cfg!(feature = "profile"),
        "launcher-response-pmu-v1": true,
        "pmu-probe-v1": true,
        "pmu-profile-v1": true,
        "pmu-profile-v2": true,
        "persisted-search-v1": true,
        "input-integrity-driver-v1": true,
        "arcade-velocity-scroll-v1": true,
        "system-entry-v1": true,
        "system-entry-profile-v1": cfg!(feature = "profile"),
    });
    capabilities
        .as_object_mut()
        .expect("benchmark capabilities must be an object")
        .insert(
            mister_magik_agent_protocol::SCREENSAVER_FRAME_EVIDENCE_CAPABILITY.to_owned(),
            serde_json::Value::Bool(cfg!(feature = "profile")),
        );
    capabilities
}

fn run_catalog_v3_inspect() {
    match mister_magik_catalog::catalog_acceptance::inspect_production_catalog() {
        Ok(report) => crate::ui_log!("{report}"),
        Err(error) => {
            crate::ui_errln!("catalog_v3_summary_tsv\tvalid=0\terror={error}");
            std::process::exit(1);
        }
    }
}

fn run_catalog_v3_registry_report() {
    match mister_magik_catalog::catalog_acceptance::inspect_production_registry() {
        Ok(report) => crate::ui_log!("{report}"),
        Err(error) => {
            crate::ui_errln!("catalog_v3_registry_summary_tsv\tvalid=0\terror={error}");
            std::process::exit(1);
        }
    }
}

fn dispatch_fpga(
    cmd: &str,
    f: &mut Fpga,
    process_entry_cpu_profile: Option<cpu_profile::CpuProfiler>,
    _fault_config: Option<&mister_magik_catalog::fs_fault::FaultConfig>,
    process_config: &mister_magik_fb::process_config::ProcessConfig,
) {
    match cmd {
        "read" => read_mode(f),
        "early-black" => early_black_route(f),
        "ui" => ui_runner::run_ui(
            f,
            process_entry_cpu_profile,
            process_config.launcher().clone(),
        ),
        #[cfg(mister_bench_scenes)]
        "scenes" => ui_runner::print_scenes(),
        #[cfg(mister_experiments)]
        "effects" => ui_runner::print_effects(),
        #[cfg(mister_experiments)]
        "effect-bench" => ui_effect_bench::run_effect_bench(f),
        #[cfg(feature = "diagnostics")]
        "input" => run_input(),
        "fpga-latch-report" => run_fpga_latch_report(),
        "latch-readiness-report" => {
            run_latch_readiness_report(f, process_config.diagnostics().latch_readiness_json)
        }
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

#[cfg(mister_experiments)]
fn print_experiment_capabilities() {
    #[cfg(mister_experiments)]
    {
        crate::ui_logln!("experiments=1");
        crate::ui_logln!(
            "commands=effects,camera-effects,sprite-effects,text-effects,raster-effects,transition-effects,effect-bench"
        );
    }
    #[cfg(not(mister_experiments))]
    {
        crate::ui_logln!("experiments=0");
        crate::ui_logln!("commands=");
    }
}

fn run_library_refresh() {
    let result = mister_magik_catalog::builder_service::run_with_execution_policy_and_fault_control(
        mister_magik_catalog::builder_service::BuilderOperation::Rebuild,
        mister_magik_catalog::builder_service::BuilderExecutionPolicy::ForegroundUntilFirstVisible,
        Box::new(mister_magik_mister_runtime::direct_reset_fault::process_fault_control()),
        |event| crate::ui_logln!("{}", serde_json::to_string(&event).unwrap_or_default()),
    );
    if let Err(error) = result {
        crate::ui_errln!("library_refresh\tfailed\t{error}");
        std::process::exit(1);
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

fn run_display_persist(args: &[String]) {
    let Some(mode) = args.get(2) else {
        crate::ui_errln!("display-persist requires a mode id");
        std::process::exit(2);
    };
    match mister_magik_mister_runtime::display_resolution::persist(mode) {
        Ok(()) => crate::ui_logln!("display_persist\tdone\tmode={mode}"),
        Err(error) => {
            crate::ui_errln!("display_persist\tfailed\tmode={mode}\terror={error}");
            std::process::exit(1);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PurgeLibraryDataInvocation {
    Confirmed,
    Help,
    Invalid,
}

fn purge_library_data_invocation(args: &[String]) -> PurgeLibraryDataInvocation {
    match args.get(2..) {
        Some([argument]) if argument == "--confirm" => PurgeLibraryDataInvocation::Confirmed,
        Some([argument]) if argument == "--help" || argument == "-h" => {
            PurgeLibraryDataInvocation::Help
        }
        _ => PurgeLibraryDataInvocation::Invalid,
    }
}

fn print_purge_library_data_usage() {
    crate::ui_logln!("usage: mister-magik-fb purge-library-data --confirm");
}

fn run_purge_library_data(args: &[String]) {
    match purge_library_data_invocation(args) {
        PurgeLibraryDataInvocation::Help => {
            print_purge_library_data_usage();
        }
        PurgeLibraryDataInvocation::Invalid => {
            print_purge_library_data_usage();
            crate::ui_errln!("purge-library-data requires the exact --confirm argument");
            std::process::exit(2);
        }
        PurgeLibraryDataInvocation::Confirmed => match launcher::purge_library_data() {
            Ok(outcome) => crate::ui_logln!(
                "purge_library_data\tdone\tcatalog_removed={}\tscreenshot_removed={}",
                outcome.catalog_artifacts_removed,
                outcome.screenshot_artifacts_removed
            ),
            Err(error) => {
                crate::ui_errln!("purge_library_data\tfailed\t{error}");
                std::process::exit(1);
            }
        },
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

#[cfg(test)]
const DEFAULT_LIBRARY_REFRESH_LOCK_PATH: &str = "/tmp/mister-magik/library-refresh.lock";

#[cfg(test)]
fn usable_library_database_exists(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.len() > 0)
        .unwrap_or(false)
}

#[cfg(test)]
fn library_refresh_lock_path() -> PathBuf {
    std::env::var("MISTER_LIBRARY_REFRESH_LOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_LIBRARY_REFRESH_LOCK_PATH))
}

#[cfg(test)]
enum RefreshLockState {
    Acquired(LibraryRefreshLock),
    Active { pid: u32 },
}

#[cfg(test)]
struct LibraryRefreshLock {
    path: PathBuf,
    pid: u32,
}

#[cfg(test)]
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

#[cfg(test)]
impl Drop for LibraryRefreshLock {
    fn drop(&mut self) {
        remove_pid_lock_if_owner(&self.path, self.pid);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
enum RefreshLockDecision {
    Acquired,
    Active { pid: u32 },
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
fn should_defer_parent_boot_library_refresh(
    parent_boot: bool,
    database_exists: bool,
    force_foreground: bool,
) -> bool {
    parent_boot && !database_exists && !force_foreground
}

#[cfg(test)]
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

#[cfg(test)]
fn run_catalog_inspect() {
    use mister_magik_catalog::arcade_catalog::{DEFAULT_ARCADE_ROOT, LaunchTarget};

    let args: Vec<String> = std::env::args().skip(2).collect();
    let result = (|| -> Result<String, String> {
        let action = args.first().map(String::as_str).ok_or_else(|| {
            "usage: catalog-inspect <counts|filter-options|find-launch-ref|launch-plan|prepared> ...".to_string()
        })?;
        if action == "counts" {
            return library_db::run_sqlite_inspect_cli(&[
                "SELECT (SELECT count(*) FROM game_rows) AS games,(SELECT count(*) FROM launch_target_rows) AS launch_plans,(SELECT count(*) FROM systems) AS systems,(SELECT count(*) FROM ui_arcade_preferred)+(SELECT count(*) FROM launcher_catalog_rows) AS launcher_rows".to_string(),
            ]);
        }
        if action == "launch-plan" {
            let launch_id = args
                .get(1)
                .ok_or_else(|| "catalog-inspect launch-plan needs ID".to_string())?
                .parse::<u64>()
                .map_err(|_| "catalog-inspect launch-plan ID must be an integer".to_string())?;
            return library_db::run_sqlite_inspect_cli(&[format!(
                "SELECT lt.launch_id,g.title,sys.value AS system_id,COALESCE(p.core_path,core.value) AS core_path,COALESCE(mount.value,'mount-image') AS mount_kind,COALESCE(lt.mount_index,0) AS mount_index,COALESCE(lt.delay_secs,1) AS delay_secs,CASE refkind.value WHEN 'payload' THEN 'magik-plan:payload:'||magik_path(pp.chunk_id,pp.offset,pp.len,pc.uncompressed_len,pc.bytes) WHEN 'archive' THEN 'magik-plan:archive:'||magik_path(pp.chunk_id,pp.offset,pp.len,pc.uncompressed_len,pc.bytes) WHEN 'same-payload' THEN magik_path(pp.chunk_id,pp.offset,pp.len,pc.uncompressed_len,pc.bytes) ELSE magik_path(lp.chunk_id,lp.offset,lp.len,lc.uncompressed_len,lc.bytes) END AS launch_ref,COALESCE(magik_path(pp.chunk_id,pp.offset,pp.len,pc.uncompressed_len,pc.bytes),'') AS payload_path,magik_path(sp.chunk_id,sp.offset,sp.len,sc.uncompressed_len,sc.bytes) AS source_path FROM launch_target_rows lt JOIN game_rows g ON g.game_key_id=lt.launch_id JOIN string_values sys ON sys.string_id=g.system_string_id JOIN string_values refkind ON refkind.string_id=lt.launch_ref_kind_string_id JOIN string_values core ON core.string_id=lt.core_string_id LEFT JOIN string_values profile ON profile.string_id=lt.profile_string_id LEFT JOIN profiles p ON p.profile_id=profile.value LEFT JOIN string_values mount ON mount.string_id=lt.mount_kind_string_id LEFT JOIN path_values lp ON lp.path_id=lt.launch_path_id LEFT JOIN path_chunks lc ON lc.chunk_id=lp.chunk_id LEFT JOIN path_values pp ON pp.path_id=lt.payload_path_id LEFT JOIN path_chunks pc ON pc.chunk_id=pp.chunk_id LEFT JOIN path_values sp ON sp.path_id=lt.source_path_id LEFT JOIN path_chunks sc ON sc.chunk_id=sp.chunk_id WHERE lt.launch_id={launch_id}"
            )]);
        }
        if action == "prepared" {
            let collection = args
                .get(1)
                .ok_or_else(|| "catalog-inspect prepared needs COLLECTION".to_string())?;
            let collection = collection.replace('\'', "''");
            return library_db::run_sqlite_inspect_cli(&[format!(
                "SELECT p.launch_id,g.title,s.value AS system_id,p.collection_id,p.launch_quality,p.adapter_version FROM prepared_launch_rows p JOIN game_rows g ON g.game_key_id=p.launch_id JOIN string_values s ON s.string_id=g.system_string_id WHERE p.collection_id='{collection}' ORDER BY p.launch_id"
            )]);
        }

        let sqlite_path = mister_magik_catalog::catalog_config::default_sqlite_path();
        let stamp = library_db::read_sqlite_catalog_stamp(&sqlite_path)?
            .ok_or_else(|| format!("{} has no catalog stamp", sqlite_path.display()))?;
        let loaded = library_db::load_arcade_catalog_from_navigation_projection(
            DEFAULT_ARCADE_ROOT,
            &sqlite_path,
            &stamp,
        )?
        .ok_or_else(|| "catalog navigation projection is missing or stale".to_string())?;
        let catalog = loaded.catalog;
        match action {
            "filter-options" => {
                let collection_id = args
                    .get(1)
                    .map(String::as_str)
                    .unwrap_or(mister_magik_catalog::arcade_catalog::MENU_ARCADE_SYSTEM_ID);
                let hydrated =
                    library_db::load_arcade_catalog_from_materialized_sqlite(DEFAULT_ARCADE_ROOT)?;
                let mismatches = hydrated.catalog.filter_option_mismatches(&catalog);
                let mut out = catalog_filter_inspection_tsv("navigation", collection_id, &catalog);
                out.push_str(&catalog_filter_inspection_tsv(
                    "sqlite",
                    collection_id,
                    &hydrated.catalog,
                ));
                out.push_str(&format!(
                    "catalog_filter_parity_tsv\tcollection={}\tstatus={}\tmismatched_collections={}\n",
                    sanitize_tsv_field(collection_id),
                    if mismatches.is_empty() { "ok" } else { "mismatch" },
                    mismatches.len()
                ));
                for mismatch in mismatches {
                    out.push_str(&format!(
                        "catalog_filter_mismatch_tsv\tdetail={}\n",
                        sanitize_tsv_field(&mismatch)
                    ));
                }
                Ok(out)
            }
            "find-launch-ref" => {
                let launch_refs =
                    args.get(1..)
                        .filter(|refs| !refs.is_empty())
                        .ok_or_else(|| {
                            "catalog-inspect find-launch-ref needs one or more REF values"
                                .to_string()
                        })?;
                let mut lookup = launch_refs
                    .iter()
                    .map(|launch_ref| (launch_ref.as_str(), None))
                    .collect::<std::collections::HashMap<_, _>>();
                for (index, game) in catalog.games.iter().enumerate() {
                    let target = catalog.launch_target_for_ref(&game.mra_path);
                    for (requested_ref, found) in &mut lookup {
                        if found.is_none()
                            && (game.mra_path.as_ref() == *requested_ref
                                || matches!(&target, LaunchTarget::Structured(plan) if plan.payload_path.as_ref() == *requested_ref))
                        {
                            *found = Some(index);
                        }
                    }
                }
                let header = "launch_ref\ttitle\tsystem_id\tkind\tcore_path\tpayload_path\tmount_kind\tmount_index\tdelay_secs\n";
                let mut out = header.to_string();
                for requested_ref in launch_refs {
                    let index = lookup.get(requested_ref.as_str()).copied().flatten();
                    let Some(index) = index else {
                        continue;
                    };
                    let game = &catalog.games[index];
                    let target = catalog.launch_target_for_ref(&game.mra_path);
                    let row = match target {
                        LaunchTarget::Structured(plan) => format!(
                            "{}\t{}\t{}\tstructured\t{}\t{}\t{}\t{}\t{}\n",
                            plan.launch_ref,
                            plan.title,
                            plan.system_id,
                            plan.core_path,
                            plan.payload_path,
                            plan.mount_kind,
                            plan.mount_index,
                            plan.delay_secs
                        ),
                        LaunchTarget::Prepared(selection) => format!(
                            "{}\t{}\t{}\tprepared\t\t\t\t\t\n",
                            selection.launch_ref, game.title, game.system_id
                        ),
                        LaunchTarget::Path(path) => format!(
                            "{}\t{}\t{}\tpath\t\t\t\t\t\n",
                            path, game.title, game.system_id
                        ),
                        LaunchTarget::MissingStructured(path) => format!(
                            "{}\t{}\t{}\tmissing-structured\t\t\t\t\t\n",
                            path, game.title, game.system_id
                        ),
                    };
                    out.push_str(&row);
                }
                Ok(out)
            }
            _ => Err(format!("unknown catalog-inspect action: {action}")),
        }
    })();

    match result {
        Ok(output) => crate::ui_log!("{output}"),
        Err(error) => {
            crate::ui_errln!("catalog_inspect\tfailed\t{error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
fn catalog_filter_inspection_tsv(
    source: &str,
    collection_id: &str,
    catalog: &mister_magik_catalog::arcade_catalog::ArcadeCatalog,
) -> String {
    let output_collection_id = sanitize_tsv_field(collection_id);
    let mut out = format!(
        "catalog_filter_summary_tsv\tsource={}\tcollection={}\tgames={}\tcategories={}\tdecades={}\tmanufacturers={}\tplayers={}\tcontrols={}\n",
        source,
        output_collection_id,
        catalog.system_game_count(collection_id),
        catalog.category_option_count(collection_id),
        catalog.decade_option_count(collection_id),
        catalog.manufacturer_option_count(collection_id),
        catalog.player_option_count(collection_id),
        catalog.control_option_count(collection_id)
    );
    for (dimension, options) in [
        ("category", catalog.category_options(collection_id)),
        ("decade", catalog.decade_options(collection_id)),
        ("manufacturer", catalog.manufacturer_options(collection_id)),
        ("players", catalog.player_options(collection_id)),
        ("control", catalog.control_options(collection_id)),
    ] {
        for option in options {
            out.push_str(&format!(
                "catalog_filter_option_tsv\tsource={}\tcollection={}\tdimension={}\tlabel={}\tgames={}\n",
                source,
                output_collection_id,
                dimension,
                sanitize_tsv_field(&option.label),
                option.count
            ));
        }
    }
    out
}

#[cfg(test)]
fn sanitize_tsv_field(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
}

#[cfg(feature = "diagnostics")]
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

#[cfg(feature = "diagnostics")]
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
        1920usize * 1080 * 4
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
}

#[cfg(feature = "diagnostics")]
fn run_scanout_slots_map_report() {
    match MappedRgb565Framebuffer::raw_diagnostics() {
        Ok(raw) => {
            crate::ui_logln!(
                "scanout_slots_map_fb0_tsv\tid={}\tsmem_start=0x{:x}\tsmem_len={}\tline_length={}\txres={}\tyres={}\txres_virtual={}\tyres_virtual={}\tbpp={}",
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
        Err(e) => crate::ui_logln!("scanout_slots_map_fb0_tsv\terror={e}"),
    }

    let device = OpenOptions::new()
        .read(true)
        .write(true)
        .open(SCANOUT_SLOTS_DEVICE)
        .unwrap_or_else(|e| {
            crate::ui_errln!("scanout_slots_map_report\tfailed\tstage=open\terror={e}");
            std::process::exit(1);
        });
    let layout = read_scanout_slots_layout(&device).unwrap_or_else(|e| {
        crate::ui_errln!("scanout_slots_map_report\tfailed\tstage=get_layout\terror={e}");
        std::process::exit(1);
    });
    crate::ui_logln!(
        "scanout_slots_layout_tsv\tabi_version={}\tslot_count={}\tmax_width={}\tmax_height={}\tmax_stride_bytes={}\tslot_capacity_bytes={}\tmap_bytes={}\tflags=0x{:x}",
        layout.abi_version,
        layout.slot_count,
        layout.max_width,
        layout.max_height,
        layout.max_stride_bytes,
        layout.slot_capacity_bytes,
        layout.map_bytes,
        layout.flags
    );
    for (index, slot) in layout.slots.iter().enumerate() {
        let probe = ScanoutSlotsByteRange::probe(
            slot.mmap_offset_bytes as usize,
            layout.map_bytes as usize,
        );
        let ok = probe.is_ok();
        crate::ui_logln!(
            "scanout_slots_map_mmap_tsv\tindex={index}\tphys=0x{:08x}\toffset={}\trequested_len={}\tok={}\terror={}",
            slot.physical_address,
            slot.mmap_offset_bytes,
            layout.map_bytes,
            bool_tsv(ok),
            probe.err().map(|e| e.to_string()).unwrap_or_default()
        );
        if !ok {
            std::process::exit(1);
        }
    }

    let rejection_cases = [
        (
            "invalid-index",
            2 * 1_048_576,
            layout.map_bytes as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
        ),
        (
            "oversized",
            0,
            layout.map_bytes as usize + 4096,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
        ),
        (
            "partial",
            0,
            layout.map_bytes as usize - 4096,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
        ),
        (
            "private",
            0,
            layout.map_bytes as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE,
        ),
        (
            "executable",
            0,
            layout.map_bytes as usize,
            libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
            libc::MAP_SHARED,
        ),
    ];
    for (case, offset, len, prot, flags) in rejection_cases {
        let result = ScanoutSlotsByteRange::probe_with(offset, len, prot, flags);
        let errno = result
            .as_ref()
            .err()
            .and_then(|error| error.raw_os_error())
            .unwrap_or_default();
        crate::ui_logln!(
            "scanout_slots_map_reject_tsv\tcase={case}\trejected={}\terrno={errno}\terror={}",
            bool_tsv(result.is_err()),
            result.err().map(|e| e.to_string()).unwrap_or_default()
        );
        if errno != libc::EINVAL {
            crate::ui_errln!(
                "scanout_slots_map_report\tfailed\tstage=reject_invalid_mapping\tcase={case}\terrno={errno}"
            );
            std::process::exit(1);
        }
    }
    let unknown_ioctl = unsafe { libc::ioctl(device.as_raw_fd(), 0) };
    let unknown_errno = std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or_default();
    if unknown_ioctl != -1 || unknown_errno != libc::ENOTTY {
        crate::ui_errln!(
            "scanout_slots_map_report\tfailed\tstage=reject_unknown_ioctl\tresult={unknown_ioctl}\terrno={unknown_errno}"
        );
        std::process::exit(1);
    }
}

fn publish_latch_readiness_report(
    report: &mister_magik_fb::latch_readiness::LatchReadinessReport,
    json_output: bool,
) {
    if let Err(error) = report.write_atomic(mister_magik_fb::latch_readiness::REPORT_PATH) {
        crate::ui_errln!("latch_readiness_report_write_failed\terror={error}");
        std::process::exit(50);
    }
    if json_output {
        match serde_json::to_string(report) {
            Ok(json) => crate::ui_logln!("{json}"),
            Err(error) => {
                crate::ui_errln!("latch_readiness_report_serialize_failed\terror={error}");
                std::process::exit(50);
            }
        }
    } else {
        crate::ui_logln!("{}", format_latch_readiness_tsv(report));
    }
    if report.state != mister_magik_fb::latch_readiness::LatchReadinessState::Ready {
        std::process::exit(match report.state {
            mister_magik_fb::latch_readiness::LatchReadinessState::InstallationFault => 30,
            mister_magik_fb::latch_readiness::LatchReadinessState::PlatformIncompatible => 40,
            mister_magik_fb::latch_readiness::LatchReadinessState::RuntimeFault => 50,
            mister_magik_fb::latch_readiness::LatchReadinessState::Ready => 0,
        });
    }
}

fn format_latch_readiness_tsv(
    report: &mister_magik_fb::latch_readiness::LatchReadinessReport,
) -> String {
    format!(
        "latch_readiness_tsv\tvalid={}\tstate={}\tstage={}\treason={}\tdetail={}",
        u8::from(report.state == mister_magik_fb::latch_readiness::LatchReadinessState::Ready),
        report.state.code(),
        report.stage.map_or("none", |stage| stage.code()),
        report.reason_code.as_deref().unwrap_or("none"),
        report.detail.replace(['\t', '\n', '\r'], " ")
    )
}

fn run_latch_readiness_report(fpga: &mut Fpga, json_output: bool) {
    use mister_magik_fb::latch_readiness::{
        LatchFailure, LatchFailureReason, LatchFailureStage, LatchReadinessReport,
    };

    let kernel_release = fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_else(|_| "unknown".to_string())
        .trim()
        .to_string();
    if kernel_release != mister_magik_scanout_contract::QUALIFIED_KERNEL_RELEASE {
        let failure = LatchFailure::incompatible(
            LatchFailureStage::Kernel,
            LatchFailureReason::KernelReleaseUnsupported,
            format!(
                "detected={} expected={}",
                kernel_release,
                mister_magik_scanout_contract::QUALIFIED_KERNEL_RELEASE
            ),
        );
        publish_latch_readiness_report(
            &LatchReadinessReport::failed(kernel_release, &failure),
            json_output,
        );
        return;
    }

    let device = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(mister_magik_scanout_contract::DEVICE)
    {
        Ok(device) => device,
        Err(error) => {
            let failure = LatchFailure::incompatible(
                LatchFailureStage::ModuleOpen,
                LatchFailureReason::ScanoutDeviceMissing,
                error.to_string(),
            );
            publish_latch_readiness_report(
                &LatchReadinessReport::failed(kernel_release, &failure),
                json_output,
            );
            return;
        }
    };
    let layout =
        match mister_magik_fb::framebuffer::scanout_slots::read_scanout_slots_layout(&device) {
            Ok(layout) => layout,
            Err(error) => {
                let failure = LatchFailure::incompatible(
                    LatchFailureStage::ModuleLayout,
                    LatchFailureReason::ScanoutAbiMismatch,
                    error.to_string(),
                );
                publish_latch_readiness_report(
                    &LatchReadinessReport::failed(kernel_release, &failure),
                    json_output,
                );
                return;
            }
        };

    let (caps_hi, caps_lo, caps) = match fpga.read_magik_latched_fbuf_capabilities() {
        Ok(caps) => caps,
        Err(error) => {
            let failure = LatchFailure::runtime(
                LatchFailureStage::FpgaCapabilities,
                LatchFailureReason::FpgaTransportFailed,
                error.to_string(),
            );
            publish_latch_readiness_report(
                &LatchReadinessReport::failed(kernel_release, &failure),
                json_output,
            );
            return;
        }
    };
    let caps_supported =
        caps_hi == fpga::MAGIK_FBUF_CAPS_MAGIC || caps_lo == fpga::MAGIK_FBUF_CAPS_MAGIC;
    if !caps_supported || !caps.production_ready() {
        let failure = LatchFailure::incompatible(
            LatchFailureStage::FpgaCapabilities,
            if caps_supported {
                LatchFailureReason::FpgaCapabilitiesInsufficient
            } else {
                LatchFailureReason::FpgaProtocolUnsupported
            },
            format!(
                "magic=0x{caps_hi:04x}/0x{caps_lo:04x} protocol={} flags=0x{:04x} max={}x{} stride={}",
                caps.protocol_version,
                caps.flags,
                caps.max_width,
                caps.max_height,
                caps.max_stride_bytes
            ),
        );
        publish_latch_readiness_report(
            &LatchReadinessReport::failed(kernel_release, &failure),
            json_output,
        );
        return;
    }

    let status = match fpga.read_magik_latched_fbuf_status() {
        Ok(status) if status.supported() => status,
        Ok(status) => {
            let failure = LatchFailure::incompatible(
                LatchFailureStage::FpgaStatus,
                LatchFailureReason::FpgaStatusUnsupported,
                format!("magic=0x{:04x}/0x{:04x}", status.magic_hi, status.magic_lo),
            );
            publish_latch_readiness_report(
                &LatchReadinessReport::failed(kernel_release, &failure),
                json_output,
            );
            return;
        }
        Err(error) => {
            let failure = LatchFailure::runtime(
                LatchFailureStage::FpgaStatus,
                LatchFailureReason::FpgaTransportFailed,
                error.to_string(),
            );
            publish_latch_readiness_report(
                &LatchReadinessReport::failed(kernel_release, &failure),
                json_output,
            );
            return;
        }
    };

    let mut report = LatchReadinessReport::ready(kernel_release);
    report.scanout_abi_version = Some(layout.abi_version);
    report.scanout_slot_capacity_bytes = Some(layout.slot_capacity_bytes);
    report.latch_protocol_version = Some(caps.protocol_version);
    report.latch_capability_flags = Some(caps.flags);
    report.latch_max_width = Some(caps.max_width);
    report.latch_max_height = Some(caps.max_height);
    report.latch_max_stride_bytes = Some(caps.max_stride_bytes);
    report.detail = format!(
        "live platform ready flip_count={} post_count={} drop_count={}",
        status.flip_count, status.post_count, status.drop_count
    );
    publish_latch_readiness_report(&report, json_output);
}

fn run_fpga_latch_report() {
    let mut fpga = match Fpga::open() {
        Ok(fpga) => fpga,
        Err(e) => {
            crate::ui_errln!("fpga_latch_report_failed\tstage=open_fpga\terror={e}");
            std::process::exit(1);
        }
    };
    let negotiated_caps = match fpga.read_magik_latched_fbuf_capabilities() {
        Ok(caps) => caps,
        Err(e) => {
            crate::ui_logln!(
                "fpga_latch_caps_tsv\tcmd=0x{:02x}\tsupported=0\tproduction_ready=0\tmagic_expected=0x{:04x}\terror={e}",
                fpga::MAGIK_UIO_GET_FBUF_LATCH_CAPS,
                fpga::MAGIK_FBUF_CAPS_MAGIC
            );
            std::process::exit(1);
        }
    };
    let negotiated_profile_ready = (negotiated_caps.0 == fpga::MAGIK_FBUF_CAPS_MAGIC
        || negotiated_caps.1 == fpga::MAGIK_FBUF_CAPS_MAGIC)
        && negotiated_caps.2.production_ready();
    if !negotiated_profile_ready {
        crate::ui_logln!(
            "fpga_latch_caps_tsv\tcmd=0x{:02x}\tsupported={}\tproduction_ready=0\tmagic_expected=0x{:04x}\tack_high=0x{:04x}\tack_low=0x{:04x}\tprotocol_version={}\tflags=0x{:04x}\tmax_width={}\tmax_height={}\tmax_stride_bytes={}",
            fpga::MAGIK_UIO_GET_FBUF_LATCH_CAPS,
            bool_tsv(
                negotiated_caps.0 == fpga::MAGIK_FBUF_CAPS_MAGIC
                    || negotiated_caps.1 == fpga::MAGIK_FBUF_CAPS_MAGIC
            ),
            fpga::MAGIK_FBUF_CAPS_MAGIC,
            negotiated_caps.0,
            negotiated_caps.1,
            negotiated_caps.2.protocol_version,
            negotiated_caps.2.flags,
            negotiated_caps.2.max_width,
            negotiated_caps.2.max_height,
            negotiated_caps.2.max_stride_bytes
        );
        std::process::exit(1);
    }

    let set_probe = (0, 0, String::new(), "capabilities");
    let set_supported = negotiated_profile_ready;
    crate::ui_logln!(
        "fpga_latch_set_probe_tsv\tcmd=0x{:02x}\tsupported={}\tsource={}\tmagic_expected=0x{:04x}\tack_high=0x{:04x}\tack_low=0x{:04x}\terror={}",
        fpga::MAGIK_UIO_SET_FBUF_LATCH,
        bool_tsv(set_supported),
        set_probe.3,
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

    let (caps_hi, caps_lo, caps) = negotiated_caps;
    let caps_supported =
        caps_hi == fpga::MAGIK_FBUF_CAPS_MAGIC || caps_lo == fpga::MAGIK_FBUF_CAPS_MAGIC;
    crate::ui_logln!(
        "fpga_latch_caps_tsv\tcmd=0x{:02x}\tsupported={}\tproduction_ready={}\tmagic_expected=0x{:04x}\tack_high=0x{:04x}\tack_low=0x{:04x}\tprotocol_version={}\tflags=0x{:04x}\tmax_width={}\tmax_height={}\tmax_stride_bytes={}",
        fpga::MAGIK_UIO_GET_FBUF_LATCH_CAPS,
        bool_tsv(caps_supported),
        bool_tsv(caps_supported && caps.production_ready()),
        fpga::MAGIK_FBUF_CAPS_MAGIC,
        caps_hi,
        caps_lo,
        caps.protocol_version,
        caps.flags,
        caps.max_width,
        caps.max_height,
        caps.max_stride_bytes
    );
    if !set_supported || !status.supported() || !caps_supported || !caps.production_ready() {
        std::process::exit(1);
    }
}

#[cfg(all(feature = "diagnostics", feature = "ui"))]
fn require_diagnostic_latch_capabilities(
    fpga: &mut Fpga,
    command: &str,
) -> mister_magik_latch_contract::LatchCapabilities {
    let (magic_hi, magic_lo, capabilities) = fpga
        .read_magik_latched_fbuf_capabilities()
        .unwrap_or_else(|error| {
            crate::ui_errln!("{command}_failed\tstage=capabilities\terror={error}");
            std::process::exit(1);
        });
    let supported =
        magic_hi == fpga::MAGIK_FBUF_CAPS_MAGIC || magic_lo == fpga::MAGIK_FBUF_CAPS_MAGIC;
    if !supported || !capabilities.production_ready() {
        crate::ui_errln!(
            "{command}_failed\tstage=capabilities\tmagic=0x{magic_hi:04x}/0x{magic_lo:04x}\tprotocol={}\tflags=0x{:04x}",
            capabilities.protocol_version,
            capabilities.flags
        );
        std::process::exit(1);
    }
    capabilities
}

#[cfg(all(feature = "diagnostics", feature = "ui"))]
fn run_fpga_latch_post_report(fpga: &mut Fpga) {
    use slint::platform::software_renderer::Rgb565Pixel;

    let _capabilities = require_diagnostic_latch_capabilities(fpga, "fpga_latch_post_report");
    let width = 960usize;
    let height = 540usize;
    let stride_bytes = rgb565_stride_bytes(width);
    let mut buffer = match ScanoutSlotsRgb565Framebuffer::open(
        HiddenRgb565BufferIndex::new(1).expect("hidden slot 1 index"),
        width,
        height,
        stride_bytes,
    ) {
        Ok(buffer) => buffer,
        Err(e) => {
            crate::ui_errln!(
                "fpga_latch_post_report_failed\tstage=open_scanout_slots_buffer\terror={e}"
            );
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

    let _capabilities = require_diagnostic_latch_capabilities(fpga, "fpga_latch_pattern");
    let frames = std::env::var("MISTER_FPGA_LATCH_PATTERN_FRAMES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| std::env::args().nth(2).and_then(|value| value.parse().ok()))
        .unwrap_or(180)
        .max(1);
    let width = 960usize;
    let height = 540usize;
    let stride_bytes = rgb565_stride_bytes(width);
    let mut buffer1 = match ScanoutSlotsRgb565Framebuffer::open(
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
    let mut buffer2 = match ScanoutSlotsRgb565Framebuffer::open(
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
    let period_us = std::env::var("MISTER_FPGA_LATCH_PATTERN_PERIOD_US")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(16_667);
    let period = std::time::Duration::from_micros(period_us);
    let mut next_deadline = std::time::Instant::now();

    crate::ui_logln!(
        "fpga_latch_pattern_header_tsv\tframes={frames}\tperiod_us={period_us}\twidth={width}\theight={height}\tstride_bytes={stride_bytes}\tbytes_per_frame={}\tbuffer1_phys=0x{base1:08x}\tbuffer2_phys=0x{base2:08x}",
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

fn bool_tsv(value: bool) -> &'static str {
    if value { "1" } else { "0" }
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
struct ScanoutSlotsByteRange {
    mem: *mut u8,
    len: usize,
    _device: File,
}

#[cfg(feature = "diagnostics")]
impl ScanoutSlotsByteRange {
    fn probe(offset: usize, len: usize) -> std::io::Result<()> {
        Self::open(offset, len).map(|_| ())
    }

    fn probe_with(offset: usize, len: usize, prot: i32, flags: i32) -> std::io::Result<()> {
        Self::open_with(offset, len, prot, flags).map(|_| ())
    }

    fn open(offset: usize, len: usize) -> std::io::Result<Self> {
        Self::open_with(
            offset,
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
        )
    }

    fn open_with(offset: usize, len: usize, prot: i32, flags: i32) -> std::io::Result<Self> {
        if len == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "zero-length scanout-slot range",
            ));
        }
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(SCANOUT_SLOTS_DEVICE)?;
        // SAFETY: fd refers to the scanout slots misc device; mapping length is
        // requested by diagnostics and unmapped in Drop.
        let mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                prot,
                flags,
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
                "scanout-slot mmap returned null",
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
impl Drop for ScanoutSlotsByteRange {
    fn drop(&mut self) {
        // SAFETY: mem/len come from successful mmap and are unmapped once here.
        unsafe {
            libc::munmap(self.mem.cast::<libc::c_void>(), self.len);
        }
    }
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

#[cfg(feature = "diagnostics")]
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

#[cfg(feature = "diagnostics")]
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
    let mister_bin = mister_magik_catalog::device_layout::DeviceLayout::current().main_path();
    crate::ui_logln!("core handoff → {mister_bin} {}", args[1..].join(" "));
    let c_path = CString::new(mister_bin).expect("CString");
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
    crate::ui_errln!("execv({mister_bin}) failed: {err}");
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

#[cfg(feature = "diagnostics")]
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

#[cfg(feature = "diagnostics")]
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
    fn destructive_library_purge_requires_exact_confirmation() {
        let args = |tail: &[&str]| {
            ["mister-magik-fb", "purge-library-data"]
                .into_iter()
                .chain(tail.iter().copied())
                .map(str::to_string)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            purge_library_data_invocation(&args(&["--confirm"])),
            PurgeLibraryDataInvocation::Confirmed
        );
        assert_eq!(
            purge_library_data_invocation(&args(&["--help"])),
            PurgeLibraryDataInvocation::Help
        );
        for invalid in [
            args(&[]),
            args(&["confirm"]),
            args(&["--confirm", "extra"]),
            args(&["--force"]),
        ] {
            assert_eq!(
                purge_library_data_invocation(&invalid),
                PurgeLibraryDataInvocation::Invalid
            );
        }
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
    fn catalog_filter_inspection_reports_production_option_counts() {
        use mister_magik_catalog::arcade_catalog::{ArcadeCatalog, ArcadeGameEntry};

        let games = ["Shooter", "Maze"]
            .into_iter()
            .enumerate()
            .map(|(index, control)| ArcadeGameEntry {
                title: format!("Game {index}").into(),
                mra_path: format!("/games/{index}.mra").into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "arcade".into(),
                year: Some(1980 + index as u16 * 10),
                manufacturer: ["Capcom", "Sega"][index].into(),
                category: ["Shooter", "Maze"][index].into(),
                players: Some((index + 1) as u8),
                control: control.into(),
                is_new: false,
            })
            .collect();
        let catalog = ArcadeCatalog::new(PathBuf::from("/games"), games, Vec::new());

        let output = catalog_filter_inspection_tsv("navigation", "arcade", &catalog);

        assert!(output.contains(
            "catalog_filter_summary_tsv\tsource=navigation\tcollection=arcade\tgames=2\tcategories=2\tdecades=2\tmanufacturers=2\tplayers=2\tcontrols=2"
        ));
        assert!(output.contains(
            "catalog_filter_option_tsv\tsource=navigation\tcollection=arcade\tdimension=category\tlabel=Maze\tgames=1"
        ));
        assert!(output.contains(
            "catalog_filter_option_tsv\tsource=navigation\tcollection=arcade\tdimension=control\tlabel=Maze\tgames=1"
        ));
        assert_eq!(sanitize_tsv_field("one\ttwo\nthree"), "one two three");
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

    #[test]
    fn latch_readiness_tsv_is_compact_and_sanitized() {
        let mut report = mister_magik_fb::latch_readiness::LatchReadinessReport::ready(
            "5.15.1-MiSTer".to_string(),
        );
        report.detail = "live platform ready\tflip_count=4\npost_count=5 drop_count=0".to_string();
        assert_eq!(
            format_latch_readiness_tsv(&report),
            "latch_readiness_tsv\tvalid=1\tstate=ready\tstage=none\treason=none\tdetail=live platform ready flip_count=4 post_count=5 drop_count=0"
        );
    }

    #[test]
    fn benchmark_capabilities_preserve_pmu_v1_and_advertise_v2() {
        let capabilities = benchmark_capabilities();
        assert_eq!(capabilities["pmu-profile-v1"], true);
        assert_eq!(capabilities["pmu-profile-v2"], true);
        assert_eq!(capabilities["settings-navigation-transition-v4"], true);
    }
}
