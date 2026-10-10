// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Native MiSTer MagiK framebuffer frontend.
//!
//! Subcommands:
//!   Production:
//!     ui [scene] [secs]  Slint UI (default `launcher`, infinite when secs=0)
//!     early-black        route a black launcher framebuffer before full UI
//!     library-refresh    update the installed catalog or build it when missing
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
//!     catalog-inspect    validate the registry, system artifacts, and source snapshot
//!     catalog-corpus-inventory inventory production-planned scan targets only
//!     catalog-registry-report list system counts without opening system artifacts
//!     catalog-screenshot-audit
//!                        reconcile installed screenshot identity coverage for one system
//!     preview-render-probe
//!                        decode one indexed screenshot and exercise production composition
//!     metadata-qualification-report
//!                        report compact-only metadata probes and device acceptance steps
//!     search-bench       benchmark persisted Arcade FTS5 search
//!   Bench tools (`--features bench-tools`):
//!     media-bench-download
//!                        benchmark raw screenshot pack persistence
//!     media-bench-save   benchmark screenshot pack save/publish paths
//!     preview-pack-bench benchmark screenshot pack entry access/decode timings
//!     framebuffer-stream-scalar-bench
//!                        measure the production RGB565 scalar decimator
//!     input              gamepad log / sniff / calibrate
//!   Benchmarks:
//!     scenes             list Slint scene names
//!   Experiments:
//!     effects            list framebuffer effect benchmark names
//!     preview-transitions list screenshot transition labels
//!     effect-bench       run framebuffer effect benchmarks
//!
//! Game/core launch requests must go through MiSTer_MagiK supervision.
//!
//! See docs/architecture.md for display routing and boot handoff; see
//! apps/mister/BUILD.md for toolchain details.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
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

pub use mister_magik_fb::{
    arcade_button_overrides, arcade_catalog, command_args, controller_db, framebuffer, input_event,
    input_repeat, input_state, launcher, launcher_presentation, launcher_taxonomy, licenses,
    media_update, particle_engine, preview_worker, return_catalog_capsule, setup_nav,
    spring_animation, ui_errln, ui_log, ui_logln,
};
use mister_magik_fb::{media_bench_download, search_bench, ui_display, ui_runner};

use fpga::{Fpga, MAGIK_FBUF_LATCH_MAGIC, MAGIK_FBUF_STATUS_MAGIC, UIO_GET_FB_PAR, UIO_GET_VRES};
use mister_magik_fb::framebuffer::format::{production_label, rgb565_stride_bytes};
use mister_magik_fb::framebuffer::mapped::MappedRgb565Framebuffer;
use mister_magik_fb::framebuffer::ownership::DisplayOwnerLock;
use ui_display::{UiDisplay, UiDisplayPlan};
use ui_runner::launcher_display_session::LauncherDisplaySession;
use ui_runner::ui_boot::{detect_runtime_display_geometry_for_plan, settle_boot_black_frame};

const DEFAULT_PROCESS_LOCK_PATH: &str = "/tmp/mister-magik/process.lock";
pub fn run() {
    let _ = process_start_monotonic_us();
    let args: Vec<String> = std::env::args().collect();
    mister_magik_fb::crash_report::install_panic_hook(args.clone());
    let build_identity = build_identity::BuildIdentity::current();
    boot_analytics::event(
        "process_start",
        format!("args={} {}", args.join(" "), build_identity.log_detail()),
    );

    if args.len() >= 2 && command_args::is_launchable_arg(&args[1]) {
        reject_direct_launch_arg(&args[1]);
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
        dispatch_pre_fpga(&cmd, &args, fault_config.as_ref(), &process_config);
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

    dispatch_fpga(&cmd, &mut f, fault_config.as_ref(), &process_config);
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
    process_config: &mister_magik_fb::process_config::ProcessConfig,
) {
    match cmd {
        "library-refresh" => run_library_refresh(process_config.catalog_paths()),
        "request-library-rebuild" => run_request_library_rebuild(),
        "toggle-simple-joystick-setting" => run_toggle_simple_joystick_setting(),
        "display-persist" => run_display_persist(args),
        "purge-library-data" => run_purge_library_data(args),
        "reset-delete-screenshot-packs" => run_reset_delete_screenshot_packs(args),
        "search-bench" => search_bench::run(),
        command_args::CATALOG_CORPUS_INVENTORY_COMMAND => run_catalog_corpus_inventory(),
        "media-bench-download" => media_bench_download::run(),
        command_args::CATALOG_INSPECT_COMMAND => {
            run_catalog_inspect(process_config.catalog_paths())
        }
        command_args::CATALOG_NEOGEO_FAMILY_AUDIT_COMMAND => {
            run_catalog_neogeo_family_audit(process_config.device_paths().device_root())
        }
        command_args::CATALOG_REGISTRY_REPORT_COMMAND => {
            run_catalog_registry_report(process_config.catalog_paths())
        }
        command_args::CATALOG_SCREENSHOT_AUDIT_COMMAND => run_catalog_screenshot_audit(
            process_config.catalog_paths(),
            args.get(2..).unwrap_or_default(),
        ),
        command_args::PREVIEW_RENDER_PROBE_COMMAND => run_preview_render_probe(
            process_config.catalog_paths(),
            args.get(2..).unwrap_or_default(),
        ),
        command_args::RUNTIME_METADATA_QUALIFICATION_COMMAND => {
            run_runtime_metadata_qualification_report()
        }
        command_args::CATALOG_WORKER_COMMAND => ui_runner::run_catalog_worker_child(args),
        other => unknown_command(other),
    }
}

fn run_catalog_corpus_inventory() {
    let roots = mister_magik_catalog::catalog_config::library_roots_from_env();
    crate::ui_log!(
        "{}",
        mister_magik_catalog::catalog_corpus_inventory_tsv(&roots)
    );
}

fn run_catalog_inspect(paths: &mister_magik_catalog::device_layout::CatalogPaths) {
    match mister_magik_catalog::catalog_acceptance::inspect_catalog(paths.sharded_catalog_dir()) {
        Ok(report) => crate::ui_log!("{report}"),
        Err(error) => {
            crate::ui_errln!("catalog_summary_tsv\tvalid=0\terror={error}");
            std::process::exit(1);
        }
    }
}

fn run_catalog_neogeo_family_audit(storage_root: &std::path::Path) {
    match mister_magik_catalog::fast_catalog_sources::audit_installed_neogeo_families(storage_root)
    {
        Ok(report) => {
            crate::ui_log!("{}", report.text);
            if !report.valid {
                std::process::exit(1);
            }
        }
        Err(error) => {
            crate::ui_errln!("neogeo_family_summary_tsv\tvalid=0\terror={error}");
            std::process::exit(1);
        }
    }
}

fn run_catalog_registry_report(paths: &mister_magik_catalog::device_layout::CatalogPaths) {
    match mister_magik_catalog::catalog_acceptance::inspect_registry(paths.sharded_catalog_dir()) {
        Ok(report) => crate::ui_log!("{report}"),
        Err(error) => {
            crate::ui_errln!("catalog_registry_summary_tsv\tvalid=0\terror={error}");
            std::process::exit(1);
        }
    }
}

fn run_runtime_metadata_qualification_report() {
    if std::env::args().nth(2).is_some() {
        crate::ui_errln!("metadata-qualification-report accepts no arguments");
        std::process::exit(2);
    }
    match mister_magik_catalog::runtime_metadata::runtime_metadata_qualification_report() {
        Ok(report) => crate::ui_log!("{report}"),
        Err(error) => {
            crate::ui_errln!(
                "metadata_qualification_report\tvalid=0\terror={}",
                sanitize_tsv_field(&error)
            );
            std::process::exit(1);
        }
    }
}

fn run_catalog_screenshot_audit(
    paths: &mister_magik_catalog::device_layout::CatalogPaths,
    args: &[String],
) {
    let Some(system) = args.first() else {
        crate::ui_errln!(
            "catalog_screenshot_summary_tsv\tvalid=0\terror=exactly one system is required"
        );
        std::process::exit(2);
    };
    if args.len() != 1 {
        crate::ui_errln!(
            "catalog_screenshot_summary_tsv\tvalid=0\terror=exactly one system is required"
        );
        std::process::exit(2);
    }
    let system_id = match mister_magik_catalog::catalog_classify::SystemId::parse(system) {
        Ok(system_id) => system_id,
        Err(error) => {
            crate::ui_errln!(
                "catalog_screenshot_summary_tsv\tvalid=0\terror={}",
                sanitize_tsv_field(&error.to_string())
            );
            std::process::exit(2);
        }
    };
    let image_size =
        mister_magik_catalog::media_identity::preferred_screenshot_image_size(system_id.as_str());
    let pack_path = match mister_magik_catalog::media_identity::size_qualified_screenshot_pack_path(
        &paths.media_asset_dir().display().to_string(),
        system_id.as_str(),
        image_size,
    ) {
        Ok(path) => PathBuf::from(path),
        Err(error) => {
            crate::ui_errln!(
                "catalog_screenshot_summary_tsv\tvalid=0\terror={}",
                sanitize_tsv_field(&error)
            );
            std::process::exit(2);
        }
    };
    let mut resolver = mister_magik_catalog::preview_availability::PreviewIdentityResolver::new();
    let outcome = match mister_magik_catalog::preview_availability::reconcile_preview_availability_with_resolver(
        paths.sharded_catalog_dir(),
        &system_id,
        &pack_path,
        mister_magik_catalog::shard_registry::production_registry_limits(),
        &mut resolver,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            crate::ui_errln!(
                "catalog_screenshot_summary_tsv\tvalid=0\tsystem={}\tpack={}\terror={}",
                system_id,
                sanitize_tsv_field(&pack_path.display().to_string()),
                sanitize_tsv_field(&error.to_string())
            );
            std::process::exit(1);
        }
    };
    crate::ui_errln!(
        "catalog_screenshot_summary_tsv\tvalid=1\tsystem={}\tgames={}\texisting_identity_rows={}\tderived_identity_rows={}\tambiguous_identity_rows={}\tcandidates={}\tavailable={}\tchanged={}\tresolver_status={:?}",
        outcome.system_id,
        outcome.games.len(),
        outcome.existing_identity_rows,
        outcome.derived_identity_rows,
        outcome.ambiguous_identity_rows,
        outcome.candidate_rows,
        outcome.available_rows,
        outcome.changed_rows,
        outcome.resolver_status,
    );
    crate::ui_log!(
        "ordinal\ttitle\tpreview_asset_key\tpreview_archive_path\thas_preview\tlaunch_ref\n"
    );
    for (ordinal, game) in outcome.games.iter().enumerate() {
        crate::ui_logln!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            ordinal,
            sanitize_tsv_field(&game.title),
            sanitize_tsv_field(&game.preview_asset_key),
            sanitize_tsv_field(&game.preview_archive_path),
            u8::from(game.has_preview),
            sanitize_tsv_field(&game.launch_ref),
        );
    }
}

fn run_preview_render_probe(
    paths: &mister_magik_catalog::device_layout::CatalogPaths,
    args: &[String],
) {
    use sha2::Digest;
    use slint::platform::software_renderer::Rgb565Pixel;

    let fail = |error: &str| -> ! {
        crate::ui_errln!(
            "preview_render_probe_tsv\tvalid=0\terror={}",
            sanitize_tsv_field(error)
        );
        std::process::exit(1);
    };
    if args.len() != 2 {
        fail("exactly one system and one asset key are required");
    }
    let system = match mister_magik_catalog::catalog_classify::SystemId::parse(&args[0]) {
        Ok(system) => system,
        Err(error) => fail(&error.to_string()),
    };
    let asset_key = args[1].trim();
    if asset_key.is_empty() || asset_key.contains(['\t', '\n', '\r']) {
        fail("asset key must be a non-empty single-line value");
    }
    let profile =
        mister_magik_catalog::media_identity::screenshot_resolution_profile(system.as_str())
            .unwrap_or_else(|| fail("system has no screenshot resolution profile"));
    let expected_size =
        mister_magik_catalog::media_identity::preferred_screenshot_image_size(system.as_str());
    let archive_path =
        match installed_qualification_archive_path(paths, system.as_str(), expected_size) {
            Ok(path) => path,
            Err(error) => fail(&error),
        };
    let index_path = mister_magik_catalog::preview_worker::preview_archive_sidecar_path_for_archive(
        &archive_path,
    );
    if !index_path.is_file() {
        fail(&format!(
            "sidecar index is missing: {}",
            index_path.display()
        ));
    }
    let loaded = match mister_magik_catalog::preview_worker::load_preview_asset_pixels_indexed_timed(
        &archive_path.display().to_string(),
        asset_key,
    ) {
        Ok(loaded) => loaded,
        Err(error) => fail(&error),
    };
    let (width, height, stride_pixels, source) = match &loaded.pixels {
        mister_magik_catalog::preview_worker::PreviewPixels::Rgb565 {
            width,
            height,
            stride_bytes,
            words,
        } => {
            if !profile.allows(*width, *height) {
                fail(&format!(
                    "unexpected preview geometry: {}x{} expected a maximized aspect fit within {}x{}{}",
                    width,
                    height,
                    profile.width,
                    profile.height,
                    if profile.rotatable {
                        " or rotated bounds"
                    } else {
                        ""
                    }
                ));
            }
            if *stride_bytes % 2 != 0 {
                fail("RGB565 stride is not word aligned");
            }
            let stride_pixels = (*stride_bytes / 2) as usize;
            let pixels = words.iter().copied().map(Rgb565Pixel).collect::<Vec<_>>();
            (*width as usize, *height as usize, stride_pixels, pixels)
        }
    };
    let frame_width = 1280;
    let frame_height = 720;
    let screen = mister_magik_fb::visual_composition::hdmi_preview_rect(frame_width, frame_height);
    let mut destination = vec![Rgb565Pixel(0); frame_width * frame_height];
    let rect = mister_magik_fb::visual_composition::compose_preview_frame(
        &mut destination,
        frame_width,
        frame_height,
        screen,
        mister_magik_fb::visual_composition::PreviewFrame {
            pixels: mister_magik_fb::visual_composition::PreviewPixels::Rgb565 {
                pixels: &source,
                stride_pixels,
            },
            source_width: width,
            source_height: height,
            display_width: width,
            display_height: height,
        },
        true,
        mister_magik_fb::visual_composition::PreviewSurface::full(frame_width),
    )
    .unwrap_or_else(|| fail("production preview composition rejected the frame"));
    let rendered = destination.iter().filter(|pixel| pixel.0 != 0).count();
    if rendered == 0 {
        fail("production preview composition produced a blank frame");
    }
    let mut hasher = sha2::Sha256::new();
    for pixel in &destination {
        hasher.update(pixel.0.to_le_bytes());
    }
    let checksum = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    crate::ui_logln!(
        "preview_render_probe_tsv\tvalid=1\tsystem={}\tasset_key={}\tarchive_path={}\tindex_path={}\tload_source={}\tsource_width={}\tsource_height={}\trender_width={}\trender_height={}\trendered_pixels={}\tpixel_sha256={}\tdecode_us={}\traw565_parse_us={}\ttotal_us={}\trect={}x{}",
        system,
        sanitize_tsv_field(asset_key),
        sanitize_tsv_field(&archive_path.display().to_string()),
        sanitize_tsv_field(&index_path.display().to_string()),
        loaded.load_source.label(),
        width,
        height,
        frame_width,
        frame_height,
        rendered,
        checksum,
        loaded.decode_us,
        loaded.raw565_parse_us,
        loaded.total_us,
        rect.width(),
        rect.rows(),
    );
}

fn installed_qualification_archive_path(
    paths: &mister_magik_catalog::device_layout::CatalogPaths,
    system: &str,
    expected_size: &str,
) -> Result<PathBuf, String> {
    let expected = mister_magik_catalog::media_identity::size_qualified_screenshot_pack_path(
        &paths.media_asset_dir().display().to_string(),
        system,
        expected_size,
    )?;
    let expected = PathBuf::from(expected);
    let state_path = mister_magik_catalog::media_identity::screenshot_media_state_path_in_root(
        paths.media_asset_dir(),
    );
    let state_text = fs::read_to_string(&state_path).map_err(|error| {
        format!(
            "read screenshot media state {}: {error}",
            state_path.display()
        )
    })?;
    let state: serde_json::Value = serde_json::from_str(&state_text).map_err(|error| {
        format!(
            "parse screenshot media state {}: {error}",
            state_path.display()
        )
    })?;
    let entry = state
        .get("systems")
        .and_then(|systems| systems.get(system))
        .ok_or_else(|| format!("screenshot media state has no {system} entry"))?;
    let state_image_size = entry.get("image_size").and_then(serde_json::Value::as_str);
    if state_image_size.is_some_and(|size| size != expected_size) {
        return Err(format!(
            "screenshot media state {system} image size is not {expected_size} (got {})",
            state_image_size.unwrap_or("missing")
        ));
    }
    let state_path_value = entry
        .get("local_path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("screenshot media state {system} has no local_path"))?;
    if Path::new(state_path_value) != expected {
        return Err(format!(
            "screenshot media state {system} points to {}, expected {}",
            state_path_value,
            expected.display()
        ));
    }
    if !expected.is_file() {
        return Err(format!(
            "screenshot archive is missing: {}",
            expected.display()
        ));
    }
    Ok(expected)
}

fn dispatch_fpga(
    cmd: &str,
    f: &mut Fpga,
    _fault_config: Option<&mister_magik_catalog::fs_fault::FaultConfig>,
    process_config: &mister_magik_fb::process_config::ProcessConfig,
) {
    match cmd {
        "read" => read_mode(f),
        "early-black" => early_black_route(f),
        "ui" => ui_runner::run_ui(
            f,
            process_config
                .launcher()
                .expect("ui command captures launcher process configuration")
                .clone(),
        ),
        #[cfg(mister_bench_scenes)]
        "scenes" => ui_runner::print_scenes(),
        "fpga-latch-report" => run_fpga_latch_report(),
        "latch-readiness-report" => {
            run_latch_readiness_report(f, process_config.diagnostics().latch_readiness_json)
        }
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

fn run_library_refresh(paths: &mister_magik_catalog::device_layout::CatalogPaths) {
    let storage_root = Path::new("/media/fat");
    let catalog_root = paths.sharded_catalog_dir();
    let _mutation_lease =
        match mister_magik_catalog::catalog_lease::CatalogMutationLease::acquire_default() {
            Ok(lease) => lease,
            Err(error) => {
                crate::ui_errln!("library_refresh\tbusy\t{error}");
                std::process::exit(75);
            }
        };
    if let Err(error) =
        mister_magik_catalog::fast_catalog_refresh::cleanup_refresh_temporary_files_with_lease(
            catalog_root,
            &_mutation_lease,
        )
    {
        crate::ui_errln!("library_refresh\trecovery_failed\t{error}");
        std::process::exit(1);
    }
    let result =
        if mister_magik_catalog::fast_catalog_refresh::read_latest_refresh_manifest(catalog_root)
            .is_ok()
        {
            let request =
                mister_magik_catalog::fast_catalog_refresh::FastCatalogRefreshRequest::Update;
            mister_magik_catalog::fast_catalog_refresh::plan_fast_refresh(
                storage_root,
                catalog_root,
                request,
            )
            .and_then(|plan| {
                mister_magik_catalog::fast_catalog_refresh::execute_planned_fast_refresh_with_lease(
                    storage_root,
                    catalog_root,
                    request,
                    plan,
                    &_mutation_lease,
                )
            })
            .and_then(|report| serde_json::to_string(&report).map_err(|error| error.to_string()))
        } else {
            mister_magik_catalog::fast_catalog_refresh::build_fresh_catalog_with_lease(
                storage_root,
                catalog_root,
                &_mutation_lease,
                |_| {},
                |_| {},
            )
            .and_then(|report| serde_json::to_string(&report).map_err(|error| error.to_string()))
        };
    match result {
        Ok(report) => crate::ui_logln!("{report}"),
        Err(error) => {
            crate::ui_errln!("library_refresh\tfailed\t{error}");
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

fn sanitize_tsv_field(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
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
    let profile = match crate::scanout_platform::current(&kernel_release) {
        Ok(profile) => profile,
        Err(error) => {
            let expected =
                if kernel_release == mister_magik_scanout_contract::DEVELOPMENT_KERNEL_RELEASE {
                    mister_magik_scanout_contract::DEVELOPMENT_PROFILE
                } else {
                    mister_magik_scanout_contract::LEGACY_PROFILE
                };
            let failure = LatchFailure::incompatible(
                LatchFailureStage::Kernel,
                LatchFailureReason::KernelReleaseUnsupported,
                error,
            );
            publish_latch_readiness_report(
                &LatchReadinessReport::failed_for_profile(kernel_release, expected, &failure),
                json_output,
            );
            return;
        }
    };

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
                &LatchReadinessReport::failed_for_profile(kernel_release, profile, &failure),
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
                    &LatchReadinessReport::failed_for_profile(kernel_release, profile, &failure),
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
                &LatchReadinessReport::failed_for_profile(kernel_release, profile, &failure),
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
            &LatchReadinessReport::failed_for_profile(kernel_release, profile, &failure),
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
                &LatchReadinessReport::failed_for_profile(kernel_release, profile, &failure),
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
                &LatchReadinessReport::failed_for_profile(kernel_release, profile, &failure),
                json_output,
            );
            return;
        }
    };

    let mut report = LatchReadinessReport::ready_for_profile(kernel_release, profile);
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

fn bool_tsv(value: bool) -> &'static str {
    if value { "1" } else { "0" }
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
            "format={} w={} h={} scan={}x{} support_flag={flag:?}",
            production_label(),
            disp.width(),
            disp.height(),
            route_mode.hact,
            route_mode.vact
        ),
    );
    crate::ui_logln!(
        "early-black: routed {} {}x{} -> {}x{} support_flag={flag:?}",
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
        let mut report = mister_magik_fb::latch_readiness::LatchReadinessReport::ready_for_profile(
            mister_magik_scanout_contract::LEGACY_KERNEL_RELEASE.to_string(),
            mister_magik_scanout_contract::LEGACY_PROFILE,
        );
        report.detail = "live platform ready\tflip_count=4\npost_count=5 drop_count=0".to_string();
        assert_eq!(
            format_latch_readiness_tsv(&report),
            "latch_readiness_tsv\tvalid=1\tstate=ready\tstage=none\treason=none\tdetail=live platform ready flip_count=4 post_count=5 drop_count=0"
        );
    }
}
