//! Native MiSTer MagiK framebuffer frontend.
//!
//! Subcommands:
//!   Production:
//!     ui [scene] [secs]  Slint UI (default `launcher`, infinite when secs=0)
//!     early-black        route a black launcher framebuffer before full UI
//!     library-refresh    build/update the SQLite library cache
//!     experiment-capabilities
//!                        print whether experimental scenes are compiled in
//!   Diagnostics:
//!     read               print live video mode + fb params
//!     route              route the current /dev/fb0 buffer 0 to HDMI
//!     fb                 paint + optionally route current fb size
//!     vsync-probe        print per-frame vsync/fallback pacing diagnostics
//!     cpu-profile-smoke  burn CPU and verify profiler SVG output
//!     library-sql        inspect the SQLite library cache without sqlite3(1)
//!     hbmame-metadata-from-library
//!                        build supplemental HBMAME metadata from parsed MRA parents
//!     media-bench-download
//!                        benchmark screenshot pack downloads and variant decoding
//!     media-bench-save   benchmark screenshot pack save/publish paths
//!     input              gamepad log / sniff / calibrate
//!     audio-tone         play a 48 kHz stereo sine wave through /dev/MrAudio
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

#![cfg_attr(not(feature = "diagnostics"), allow(dead_code))]

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

mod arcade_list_renderer;
mod artifact_publish;
mod bitmap_text;
mod boot_analytics;
mod cpu_profile;
mod display_config;
#[cfg(mister_experiments)]
mod experiments;
mod fb;
mod fpga;
#[cfg(mister_bench_scenes)]
mod frame_profile;
mod input;
mod launch_preparation;
mod launcher;
mod media_bench_download;
mod media_bench_save;
mod media_pack_save;
mod mr_audio;
mod preview_state;
mod runtime_status;
mod screenshot_transitions;
mod ui_display;
#[cfg(mister_experiments)]
mod ui_effect_bench;
mod ui_runner;
#[cfg(feature = "video")]
mod video_player;
mod vt;

pub use mister_magik_fb::fb_format;
pub use mister_magik_fb::{
    arcade_catalog, command_args, controller_db, input_repeat, input_state, library_db,
    media_update, preview_worker, setup_nav,
};

use fb::{Display, Pixel, VsyncPacer, VsyncWaitStatus};
use fpga::{Fpga, UIO_GET_FB_PAR, UIO_GET_VRES};
use mister_magik_fb::fb_format::FramebufferFormat;
use slint::platform::software_renderer::{Rgb565Pixel, TargetPixel};
use ui_display::UiDisplayPlan;
use ui_runner::ui_boot::{
    boot_framebuffer_format, detect_runtime_display_geometry_for_plan, settle_boot_black_frame,
    FpgaFramebufferRoute,
};

const MISTER_BIN: &str = "/media/fat/MiSTer_MagiK";
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

    let cmd = command_args::resolve_command(&args);

    if cmd != "library-sql" {
        println!("mister-magik-fb [{cmd}] (arch={})", std::env::consts::ARCH);
    }

    #[cfg(feature = "diagnostics")]
    if cmd == "vsync-probe" {
        run_vsync_probe();
        return;
    }

    #[cfg(feature = "diagnostics")]
    if cmd == "cpu-profile-smoke" {
        run_cpu_profile_smoke();
        return;
    }

    if cmd == "library-refresh" {
        run_library_refresh();
        return;
    }

    if cmd == "media-bench-download" {
        media_bench_download::run();
        return;
    }

    if cmd == "media-bench-save" {
        media_bench_save::run();
        return;
    }

    #[cfg(feature = "diagnostics")]
    if cmd == "library-sql" {
        run_library_sql();
        return;
    }

    #[cfg(feature = "diagnostics")]
    if cmd == "hbmame-metadata-from-library" {
        run_hbmame_metadata_from_library();
        return;
    }

    if cmd == "launch-prep-bench" {
        launch_preparation::run_launch_prep_bench();
        return;
    }

    if cmd == "experiment-capabilities" {
        print_experiment_capabilities();
        return;
    }

    #[cfg(mister_experiments)]
    if cmd == "preview-transitions" {
        print_preview_transitions();
        return;
    }

    #[cfg(mister_experiments)]
    if cmd == "camera-effects" {
        ui_runner::print_camera_effects();
        return;
    }

    #[cfg(mister_experiments)]
    if cmd == "sprite-effects" {
        ui_runner::print_sprite_effects();
        return;
    }

    #[cfg(mister_experiments)]
    if cmd == "text-effects" {
        ui_runner::print_text_effects();
        return;
    }

    #[cfg(mister_experiments)]
    if cmd == "raster-effects" {
        ui_runner::print_raster_effects();
        return;
    }

    #[cfg(mister_experiments)]
    if cmd == "transition-effects" {
        ui_runner::print_transition_effects();
        return;
    }

    if !command_args::COMMANDS.contains(&cmd.as_str()) {
        unknown_command(&cmd);
    }

    let mut f = match Fpga::open() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("failed to open FPGA (/dev/mem): {e}");
            std::process::exit(1);
        }
    };

    match cmd.as_str() {
        #[cfg(feature = "diagnostics")]
        "read" => read_mode(&mut f),
        #[cfg(feature = "diagnostics")]
        "route" => route_framebuffer(&mut f),
        #[cfg(feature = "diagnostics")]
        "fb" => fb_probe(&mut f),
        #[cfg(feature = "diagnostics")]
        "fb-format-smoke" => fb_format_smoke(&mut f),
        "early-black" => early_black_route(&mut f),
        "ui" => ui_runner::run_ui(&mut f),
        #[cfg(mister_bench_scenes)]
        "scenes" => ui_runner::print_scenes(),
        #[cfg(mister_experiments)]
        "effects" => ui_runner::print_effects(),
        #[cfg(mister_experiments)]
        "effect-bench" => ui_effect_bench::run_effect_bench(&mut f),
        #[cfg(feature = "diagnostics")]
        "input" => run_input(),
        #[cfg(feature = "diagnostics")]
        "library-scan-bench" => library_db::run_scan_bench(),
        #[cfg(feature = "diagnostics")]
        "audio-tone" => run_audio_tone(&mut f),
        other => unknown_command(other),
    }
}

fn unknown_command(cmd: &str) -> ! {
    eprintln!(
        "unknown command '{cmd}' (use: {})",
        command_args::COMMANDS.join(" | ")
    );
    std::process::exit(2);
}

fn reject_direct_launch_arg(arg: &str) -> ! {
    eprintln!(
        "direct launch argument '{arg}' is unsupported; launch games through MiSTer_MagiK supervision"
    );
    std::process::exit(2);
}

#[cfg(mister_experiments)]
fn print_preview_transitions() {
    println!(
        "{}",
        screenshot_transitions::PreviewTransitionEffect::labels()
    );
}

fn print_experiment_capabilities() {
    #[cfg(mister_experiments)]
    {
        println!("experiments=1");
        println!("commands=effects,camera-effects,sprite-effects,text-effects,raster-effects,transition-effects,effect-bench");
    }
    #[cfg(not(mister_experiments))]
    {
        println!("experiments=0");
        println!("commands=");
    }
}

fn run_library_refresh() {
    let parent_boot = std::env::var_os("MISTER_MAGIK_PARENT").is_some();
    let database_exists = usable_library_database_exists(&library_db::default_sqlite_path());
    let force_foreground = std::env::var_os("MISTER_MAGIK_FOREGROUND_LIBRARY_REFRESH").is_some();
    if should_defer_parent_boot_library_refresh(parent_boot, database_exists, force_foreground) {
        println!("library_refresh\tdeferred\tmissing_database_parent_boot");
        return;
    }
    let lock_path = library_refresh_lock_path();
    let lock = match LibraryRefreshLock::acquire(&lock_path) {
        Ok(RefreshLockState::Acquired(lock)) => lock,
        Ok(RefreshLockState::Active { pid }) => {
            println!("library_refresh\tskipped\tactive_pid={pid}");
            return;
        }
        Err(e) => {
            eprintln!("library_refresh\tfailed\tlock {e}");
            std::process::exit(1);
        }
    };
    let mut progress = |title: &str, detail: &str| {
        println!("library_refresh\tprogress\t{title}\t{detail}");
    };
    match library_db::rebuild_default_sqlite_database(Some(&mut progress)) {
        Ok(summary) => {
            let launch_cache =
                launch_preparation::materialize_virtual_launch_cache_from_default_db();
            drop(lock);
            println!(
                "library_refresh\tdone\tskipped={} bytes={} scan_us={} discover_us={} classify_us={} import_us={} discoveries={} normal_files={} containers={} entries={} virtual_launch_total={} virtual_launch_written={} virtual_launch_unchanged={} virtual_launch_errors={}",
                summary.skipped,
                summary.bytes,
                summary.scan_us,
                summary.discover_us,
                summary.classify_us,
                summary.import_us,
                summary.discoveries,
                summary.normal_files,
                summary.containers,
                summary.entries,
                launch_cache.total,
                launch_cache.written,
                launch_cache.unchanged,
                launch_cache.errors
            );
        }
        Err(e) => {
            drop(lock);
            eprintln!("library_refresh\tfailed\t{e}");
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
        let should_remove = read_lock_pid(&self.path)
            .map(|pid| pid == self.pid)
            .unwrap_or(false);
        if should_remove {
            let _ = fs::remove_file(&self.path);
        }
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    match create_lock_file(path, pid) {
        Ok(()) => return Ok(RefreshLockDecision::Acquired),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("create {}: {e}", path.display())),
    }
    if let Some(active_pid) =
        read_lock_pid(path).filter(|locked_pid| is_active_refresh(*locked_pid))
    {
        return Ok(RefreshLockDecision::Active { pid: active_pid });
    }
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove stale {}: {e}", path.display())),
    }
    match create_lock_file(path, pid) {
        Ok(()) => Ok(RefreshLockDecision::Acquired),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if let Some(active_pid) =
                read_lock_pid(path).filter(|locked_pid| is_active_refresh(*locked_pid))
            {
                Ok(RefreshLockDecision::Active { pid: active_pid })
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
    let path = PathBuf::from(format!("/proc/{pid}/cmdline"));
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    let parts = bytes
        .split(|byte| *byte == 0)
        .filter_map(|part| std::str::from_utf8(part).ok())
        .collect::<Vec<_>>();
    parts.iter().any(|part| part.ends_with("mister-magik-fb"))
        && parts.iter().any(|part| *part == "library-refresh")
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
            print!("{output}");
            if !output.ends_with('\n') {
                println!();
            }
        }
        Err(e) => {
            eprintln!("library_sql\tfailed\t{e}");
            std::process::exit(1);
        }
    }
}

fn run_hbmame_metadata_from_library() {
    match library_db::write_default_hbmame_metadata_from_library() {
        Ok(summary) => {
            println!(
                "hbmame_metadata_from_library\tdone\tpath={}\trows={}",
                summary.path.display(),
                summary.rows
            );
        }
        Err(e) => {
            eprintln!("hbmame_metadata_from_library\tfailed\t{e}");
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
        eprintln!("cpu-profile-smoke requires MISTER_PPROF=1");
        std::process::exit(2);
    }
    println!("cpu_profile_smoke: burning CPU for {secs}s");
    let cpu = cpu_profile::start();
    if cpu.is_none() {
        eprintln!("cpu_profile_smoke: profiler did not start");
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
    println!("cpu_profile_smoke: rounds={rounds} state={state:#018x}");
    match cpu_profile::finish(cpu) {
        Ok(Some(summary)) if summary.sample_hits > 0 && summary.bytes > 0 => {
            println!(
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
            eprintln!(
                "cpu_profile_smoke: profiler produced unusable output samples={} bytes={}",
                summary.sample_hits, summary.bytes
            );
            std::process::exit(1);
        }
        Ok(None) => {
            eprintln!("cpu_profile_smoke: profiling feature is not enabled");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
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
    println!("frame\tsource\twait_us\tperiod_us\tinferred_hz\tmiss_streak\tloop_delta_us\tmessage");
    let mut last_frame_at: Option<std::time::Instant> = None;
    for frame in 0..frames {
        let frame_at = std::time::Instant::now();
        let pace = pacer.wait();
        let loop_delta_us = last_frame_at
            .map(|prev| frame_at.saturating_duration_since(prev).as_micros() as u64)
            .unwrap_or(0);
        last_frame_at = Some(frame_at);
        println!(
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
    println!(
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
    let info = match Display::current_info() {
        Ok(info) => info,
        Err(e) => {
            eprintln!("vsync-probe direct: failed to read current display info: {e}");
            std::process::exit(1);
        }
    };
    let format = FramebufferFormat::from_bits_per_pixel(info.bits_per_pixel);
    let disp =
        match Display::open_with_format(info.visible_w, info.virtual_h.max(info.visible_h), format)
        {
            Ok(d) => d,
            Err(e) => {
                eprintln!("vsync-probe direct: failed to open current display: {e}");
                std::process::exit(1);
            }
        };
    println!("frame\tsource\twait_us\tperiod_us\tinferred_hz\tmiss_streak\tloop_delta_us\tmessage");
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
        println!(
            "{frame}\t{source}\t{wait_us}\t{period_us}\t{:.2}\t{miss_streak}\t{loop_delta_us}\t{message}",
            1_000_000.0 / period_us as f64
        );
        if work_us > 0 {
            std::thread::sleep(std::time::Duration::from_micros(work_us));
        }
    }
    println!(
        "vsync_probe_summary mode=direct frames={frames} work_us={work_us} hits={hits} timeouts={timeouts} fallback_frames=0 errors={errors} max_miss_streak={max_miss_streak} inferred_hz={:.2}",
        1_000_000.0 / period_us as f64
    );
    if errors > 0 {
        std::process::exit(1);
    }
}

fn run_audio_tone(f: &mut Fpga) {
    if let Err(e) = f.set_audio_volume(0) {
        eprintln!("warning: failed to set FPGA audio volume: {e}");
    }
    let args: Vec<String> = std::env::args().skip(2).collect();
    if let Err(e) = mr_audio::run_tone_from_args(&args) {
        eprintln!("audio-tone failed: {e}");
        std::process::exit(1);
    }
}

fn exec_mister(args: &[String]) {
    println!("core handoff → {MISTER_BIN} {}", args[1..].join(" "));
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
    let err = unsafe { libc::execv(c_path.as_ptr(), ptrs.as_ptr()) };
    eprintln!("execv({MISTER_BIN}) failed: {err}");
    std::process::exit(1);
}

fn route_framebuffer(f: &mut Fpga) {
    let disp = match Display::open_current() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open current display (/dev/fb0): {e}");
            std::process::exit(1);
        }
    };
    let w = disp.width();
    let h = disp.height();
    let route = FpgaFramebufferRoute::framebuffer_sized(
        w,
        h,
        std::env::var_os("MISTER_DIRECT_VIDEO").is_some(),
        FramebufferFormat::Xrgb8888,
    );
    let flag = match route.enable(f, w, h) {
        Ok(flag) => flag,
        Err(e) => {
            eprintln!("failed to route current fb to HDMI: {e}");
            std::process::exit(1);
        }
    };
    println!("route: fb0 {w}x{h} -> HDMI support_flag={flag}");
}

fn early_black_route(f: &mut Fpga) {
    let runtime_geometry = detect_runtime_display_geometry_for_plan(f, "early-black");
    let display_plan = UiDisplayPlan::from_runtime_or_mister_ini_file(runtime_geometry);
    println!("{}", display_plan.log_line());
    if display_plan.fallback {
        boot_analytics::event("display_plan_fallback", display_plan.log_line());
    }
    let format = boot_framebuffer_format();
    if let Err(e) = Display::write_mister_mode_format(
        format,
        display_plan.fb_w,
        display_plan.fb_h,
        format.stride_bytes(display_plan.fb_w),
    ) {
        eprintln!("early-black: failed to set framebuffer mode: {e}");
        std::process::exit(1);
    }

    let mut disp = match Display::open_rgb565(display_plan.fb_w, display_plan.fb_h) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("early-black: failed to open /dev/fb0: {e}");
            std::process::exit(1);
        }
    };

    disp.clear_black();
    boot_analytics::event(
        "early_black_route_frame_copied",
        format!(
            "format={} w={} h={}",
            format.label(),
            disp.width(),
            disp.height()
        ),
    );

    let route = FpgaFramebufferRoute::for_plan_rgb565(display_plan);
    let flag = match route.enable(f, disp.width(), disp.height()) {
        Ok(flag) => flag,
        Err(e) => {
            eprintln!("early-black: failed to route framebuffer: {e}");
            std::process::exit(1);
        }
    };
    settle_boot_black_frame("early-black", &mut disp, f, route, format);
    let route_mode = route.mode();
    boot_analytics::event(
        "early_black_route_completed",
        format!(
            "format={} w={} h={} scan={}x{} support_flag={flag}",
            format.label(),
            disp.width(),
            disp.height(),
            route_mode.hact,
            route_mode.vact
        ),
    );
    println!(
        "early-black: routed {} {}x{} -> {}x{} support_flag={flag}",
        format.label(),
        disp.width(),
        disp.height(),
        route_mode.hact,
        route_mode.vact
    );
}

fn fb_probe(f: &mut Fpga) {
    let _vt = vt::VtGraphicsGuard::enter_or_warn();
    let secs = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(60);
    let route = std::env::args().nth(3).unwrap_or_else(|| "normal".into());
    let mut disp = match Display::open_current() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open current display (/dev/fb0): {e}");
            std::process::exit(1);
        }
    };
    let w = disp.width();
    let h = disp.height();
    paint_pattern(disp.buffer_mut(), w, h);
    println!("painted current {w}x{h} test pattern");

    match route.as_str() {
        "normal" => {
            let route =
                FpgaFramebufferRoute::framebuffer_sized(w, h, false, FramebufferFormat::Xrgb8888);
            let flag = match route.enable(f, w, h) {
                Ok(flag) => flag,
                Err(e) => {
                    eprintln!("failed to route current fb via SET_FBUF: {e}");
                    std::process::exit(1);
                }
            };
            println!("routed current fb via SET_FBUF only support_flag={flag}");
        }
        "direct" => {
            let route =
                FpgaFramebufferRoute::framebuffer_sized(w, h, true, FramebufferFormat::Xrgb8888);
            let flag = match route.enable(f, w, h) {
                Ok(flag) => flag,
                Err(e) => {
                    eprintln!("failed to route current fb via SET_FBUF + set_vga_fb: {e}");
                    std::process::exit(1);
                }
            };
            println!("routed current fb via SET_FBUF + set_vga_fb support_flag={flag}");
        }
        "none" => {
            println!("route skipped; expecting another owner to scan /dev/fb0");
        }
        other => {
            eprintln!("unknown fb route '{other}' (use normal|direct|none)");
            std::process::exit(2);
        }
    }

    let params = match f.read_fb_params() {
        Ok(params) => params,
        Err(e) => {
            eprintln!("failed to read framebuffer params after route: {e}");
            std::process::exit(1);
        }
    };
    println!("after route: {}", params.log_line());
    if secs == 0 {
        println!("holding forever — stop this process or reboot when done");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }
    println!("holding {secs}s — check HDMI for bordered colour test pattern...");
    std::thread::sleep(std::time::Duration::from_secs(secs));
}

fn fb_format_smoke(f: &mut Fpga) {
    let _vt = vt::VtGraphicsGuard::enter_or_warn();
    let format_arg = std::env::args().nth(2).unwrap_or_else(|| "8888".into());
    let format = match FramebufferFormat::from_label(&format_arg) {
        Some(format) => format,
        None => {
            eprintln!("fb-format-smoke format must be 8888 or 565");
            std::process::exit(2);
        }
    };
    let secs = std::env::args()
        .nth(3)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(10);
    let route = std::env::args().nth(4).unwrap_or_else(|| "normal".into());
    let previous = match Display::current_info() {
        Ok(info) => info,
        Err(e) => {
            eprintln!("failed to read current framebuffer mode: {e}");
            std::process::exit(1);
        }
    };
    const W: usize = 960;
    const H: usize = 540;
    const HDMI_W: u16 = 1920;
    const HDMI_H: u16 = 1080;
    if let Err(e) = Display::write_mister_mode_format(format, W, H, format.stride_bytes(W)) {
        eprintln!("failed to set framebuffer mode for smoke: {e}");
        std::process::exit(1);
    }
    let restore = || {
        if let Err(e) = Display::restore_mister_mode(previous) {
            eprintln!("warning: failed to restore framebuffer mode: {e}");
        }
    };
    let mut disp = match Display::open_with_format(W, H, format) {
        Ok(d) => d,
        Err(e) => {
            restore();
            eprintln!("failed to open smoke framebuffer: {e}");
            std::process::exit(1);
        }
    };
    match format {
        FramebufferFormat::Xrgb8888 => paint_pattern(disp.buffer_mut(), W, H),
        FramebufferFormat::Rgb565 => paint_pattern_565(disp.buffer_565_mut(), W, H),
    }
    let route_res = match route.as_str() {
        "normal" => FpgaFramebufferRoute::for_scan(HDMI_W, HDMI_H, false, format).enable(f, W, H),
        "direct" => FpgaFramebufferRoute::for_scan(HDMI_W, HDMI_H, true, format).enable(f, W, H),
        "none" => Ok(0),
        other => {
            restore();
            eprintln!("unknown fb-format-smoke route '{other}' (use normal|direct|none)");
            std::process::exit(2);
        }
    };
    match route_res {
        Ok(flag) => println!(
            "fb-format-smoke: format={} rb={} source={}x{} scan={}x{} stride={} route={} support_flag={flag}",
            format.label(),
            if format.route_rb() { 1 } else { 0 },
            W,
            H,
            HDMI_W,
            HDMI_H,
            format.stride_bytes(W),
            route
        ),
        Err(e) => {
            restore();
            eprintln!("failed to route smoke framebuffer: {e}");
            std::process::exit(1);
        }
    }
    if secs == 0 {
        println!("holding forever - stop this process or reboot when done");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }
    println!("holding {secs}s - check HDMI for RGB/color-ramp smoke pattern...");
    std::thread::sleep(std::time::Duration::from_secs(secs));
    restore();
}

fn paint_pattern(buf: &mut [Pixel], w: usize, h: usize) {
    const RED: u32 = 0x00FF_0000;
    const GREEN: u32 = 0x0000_FF00;
    const BLUE: u32 = 0x0000_00FF;
    const YELLOW: u32 = 0x00FF_FF00;
    const WHITE: u32 = 0x00FF_FFFF;
    const BLACK: u32 = 0x0000_0000;
    fill_rect_strided(buf, w, 0, 0, w, h, BLACK);
    fill_rect_strided(buf, w, 0, 0, w / 2, h / 2, RED);
    fill_rect_strided(buf, w, w / 2, 0, w, h / 2, GREEN);
    fill_rect_strided(buf, w, 0, h / 2, w / 2, h, BLUE);
    fill_rect_strided(buf, w, w / 2, h / 2, w, h, YELLOW);

    let b = (w.min(h) / 90).clamp(2, 8);
    fill_rect_strided(buf, w, 0, 0, w, b, WHITE);
    fill_rect_strided(buf, w, 0, h.saturating_sub(b), w, h, WHITE);
    fill_rect_strided(buf, w, 0, 0, b, h, WHITE);
    fill_rect_strided(buf, w, w.saturating_sub(b), 0, w, h, WHITE);
    fill_rect_strided(buf, w, 0, h / 2 - b / 2, w, h / 2 + b / 2, WHITE);
    fill_rect_strided(buf, w, w / 2 - b / 2, 0, w / 2 + b / 2, h, WHITE);
}

fn paint_pattern_565(buf: &mut [Rgb565Pixel], w: usize, h: usize) {
    let mut tmp = vec![Pixel(0); w * h];
    paint_pattern(&mut tmp, w, h);
    for (dst, src) in buf.iter_mut().zip(tmp) {
        let p = src.0 & 0x00ff_ffff;
        *dst = <Rgb565Pixel as TargetPixel>::from_rgb((p >> 16) as u8, (p >> 8) as u8, p as u8);
    }
}

fn fill_rect_strided(
    buf: &mut [Pixel],
    stride: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    c: u32,
) {
    for y in y0..y1 {
        for x in x0..x1 {
            buf[y * stride + x] = Pixel(c);
        }
    }
}

fn read_mode(f: &mut Fpga) {
    println!("\n=== UIO_GET_VRES (0x23) ===");
    let cmd = match f.cmd_capture(UIO_GET_VRES) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("failed to issue UIO_GET_VRES: {e}");
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
                eprintln!("failed to read UIO_GET_VRES word: {e}");
                std::process::exit(1);
            }
        };
    }
    f.disable_io();
    for (i, w) in vres.iter().enumerate() {
        print_word(&format!("  w{i:<2}"), *w);
    }
    let lo = |i: usize| vres[i].1 as u32;
    println!(
        "  -> width={} height={}",
        lo(1) | (lo(2) << 16),
        lo(3) | (lo(4) << 16)
    );

    println!("\n=== UIO_GET_FB_PAR (0x40) ===");
    let cmd = match f.cmd_capture(UIO_GET_FB_PAR) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("failed to issue UIO_GET_FB_PAR: {e}");
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
                eprintln!("failed to read UIO_GET_FB_PAR word: {e}");
                std::process::exit(1);
            }
        };
    }
    f.disable_io();
    for (i, w) in fbp.iter().enumerate() {
        print_word(&format!("  w{i:<2}"), *w);
    }
    println!(
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
    println!(
        "{label} hi=0x{:04x} ({:5})   lo=0x{:04x} ({:5})",
        w.0, w.0, w.1, w.1
    );
}

fn run_input() {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let sub = args.first().map(|s| s.as_str()).unwrap_or("log");
    match sub {
        "calibrate" => {
            let path = args.get(1).map(|s| s.as_str());
            if let Err(e) = input::calibrate(path) {
                eprintln!("input calibrate failed: {e}");
                std::process::exit(1);
            }
        }
        "log" => {
            let (path, secs) = parse_input_log_args(&args[1..]);
            if let Err(e) = input::log_js_events(path, secs) {
                eprintln!("input log failed: {e}");
                std::process::exit(1);
            }
        }
        "sniff" => {
            let (path, secs) = parse_input_log_args(&args[1..]);
            if let Err(e) = input::sniff(path, secs) {
                eprintln!("input sniff failed: {e}");
                std::process::exit(1);
            }
        }
        other => {
            eprintln!(
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
