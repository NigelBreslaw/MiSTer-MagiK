use super::launcher_frame_accounting::{
    LauncherCustomDrawTrace, LauncherFrameAccounting, LauncherPresentedFrame,
};
use super::launcher_worker_intents::{apply_launcher_worker_ui_intent, catalog_scan_message};
#[cfg(test)]
use super::launcher_worker_intents::{
    catalog_background_scan_progress_visible, catalog_scan_progress_visible,
};
use super::*;
use crate::fb::VsyncWaitStatus;
use crate::input_state::PadState;
use crate::preview_worker;
use mister_magik_catalog::catalog_summary;
use mister_magik_fb::framebuffer_ownership::{
    should_present_full_frame, FramebufferRouteAction, FramebufferRouteGuard,
};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;

const DEFAULT_LAUNCHER_REVEAL_SETTLE_FRAMES: u32 = 3;
const MAX_LAUNCHER_REVEAL_SETTLE_FRAMES: u32 = 30;
const DEFAULT_CATALOG_BACKGROUND_VALIDATION_DELAY: Duration = Duration::from_secs(2);
const LIBRARY_CHANGED_TEST_ACTION_SETTLE: Duration = Duration::from_millis(1200);
const DEFAULT_LAUNCH_HANDOFF_BENCH_DELAY: Duration = Duration::from_millis(750);
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

struct LauncherStatusTextSnapshot {
    catalog_scan_message: SharedString,
    catalog_scan_title: SharedString,
    catalog_scan_detail: SharedString,
    confirm_title: SharedString,
    confirm_left_label: SharedString,
    confirm_right_label: SharedString,
}

impl LauncherStatusTextSnapshot {
    fn from_bridge(bridge: &slint_ui::launcher::MisterBridge<'_>) -> Self {
        Self {
            catalog_scan_message: bridge.get_catalog_scan_message(),
            catalog_scan_title: bridge.get_catalog_scan_title(),
            catalog_scan_detail: bridge.get_catalog_scan_detail(),
            confirm_title: bridge.get_confirm_title(),
            confirm_left_label: bridge.get_confirm_left_label(),
            confirm_right_label: bridge.get_confirm_right_label(),
        }
    }

    fn bytes_len(&self) -> usize {
        self.catalog_scan_message.len()
            + self.catalog_scan_title.len()
            + self.catalog_scan_detail.len()
            + self.confirm_title.len()
            + self.confirm_left_label.len()
            + self.confirm_right_label.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LibraryChangedDialogTestPhase {
    Waiting,
    RebuildReleaseRight,
    RebuildPressA,
    Done,
}

struct LibraryChangedDialogTestDriver {
    choice: Option<launcher::LibraryChangedTestDialogChoice>,
    dialog_seen_at: Option<Instant>,
    phase: LibraryChangedDialogTestPhase,
}

impl LibraryChangedDialogTestDriver {
    fn from_env(start: Instant) -> Self {
        let choice = library_changed_test_dialog_choice_from_env(start);
        Self {
            choice,
            dialog_seen_at: None,
            phase: LibraryChangedDialogTestPhase::Waiting,
        }
    }

    fn input_for(&mut self, nav: &LauncherNav, now: Instant, start: Instant) -> Option<PadState> {
        let choice = self.choice?;
        if nav.confirm_action != Some(launcher::ConfirmAction::LibraryChanged) {
            self.dialog_seen_at = None;
            return None;
        }
        let seen_at = *self.dialog_seen_at.get_or_insert(now);
        if now.duration_since(seen_at) < LIBRARY_CHANGED_TEST_ACTION_SETTLE {
            return None;
        }

        match choice {
            launcher::LibraryChangedTestDialogChoice::Continue => {
                if self.phase != LibraryChangedDialogTestPhase::Waiting {
                    return None;
                }
                self.phase = LibraryChangedDialogTestPhase::Done;
                print_startup_event(
                    start,
                    "library_changed_test_dialog_input",
                    "choice=continue button=a",
                );
                Some(pad_state_with(|state| state.btn_a = true))
            }
            launcher::LibraryChangedTestDialogChoice::Rebuild => match self.phase {
                LibraryChangedDialogTestPhase::Waiting => {
                    self.phase = LibraryChangedDialogTestPhase::RebuildReleaseRight;
                    print_startup_event(
                        start,
                        "library_changed_test_dialog_input",
                        "choice=rebuild button=right",
                    );
                    Some(pad_state_with(|state| state.dpad_right = true))
                }
                LibraryChangedDialogTestPhase::RebuildReleaseRight => {
                    self.phase = LibraryChangedDialogTestPhase::RebuildPressA;
                    Some(PadState::default())
                }
                LibraryChangedDialogTestPhase::RebuildPressA => {
                    self.phase = LibraryChangedDialogTestPhase::Done;
                    print_startup_event(
                        start,
                        "library_changed_test_dialog_input",
                        "choice=rebuild button=a",
                    );
                    Some(pad_state_with(|state| state.btn_a = true))
                }
                LibraryChangedDialogTestPhase::Done => None,
            },
        }
    }
}

fn pad_state_with(set: impl FnOnce(&mut PadState)) -> PadState {
    let mut state = PadState::default();
    set(&mut state);
    state
}

struct PendingLaunch {
    rx: mpsc::Receiver<LaunchWorkerResult>,
    action_start: Instant,
    loading_presented: Instant,
    bench_iteration: Option<usize>,
    loading_frames: u64,
    max_frame_gap_us: u64,
    last_loop_start: Option<Instant>,
}

fn catalog_from_summary(
    root: &str,
    summary: &catalog_summary::CatalogSummaryProjection,
) -> ArcadeCatalog {
    let systems = summary
        .systems
        .iter()
        .map(|system| arcade_catalog::GameSystemEntry {
            id: system.id.clone(),
            title: system.title.clone(),
            count: system.count,
        })
        .collect();
    ArcadeCatalog::new(PathBuf::from(root), Vec::new(), systems)
}

fn read_catalog_summary_seed(
    sqlite_path: &Path,
    summary_path: &Path,
    start: Instant,
) -> Option<catalog_summary::CatalogSummaryProjection> {
    let summary_t = Instant::now();
    if !sqlite_path.exists() {
        print_startup_event(
            start,
            "catalog_summary_load",
            format!(
                "status=sqlite_missing elapsed_us={} sqlite_path={} path={}",
                summary_t.elapsed().as_micros(),
                sqlite_path.display(),
                summary_path.display()
            ),
        );
        return None;
    }
    if !sqlite_file_has_valid_header(sqlite_path) {
        print_startup_event(
            start,
            "catalog_summary_load",
            format!(
                "status=sqlite_unusable elapsed_us={} sqlite_path={} path={}",
                summary_t.elapsed().as_micros(),
                sqlite_path.display(),
                summary_path.display()
            ),
        );
        return None;
    }

    match catalog_summary::read_catalog_summary(summary_path) {
        Ok(Some(summary)) if !summary.systems.is_empty() => {
            print_startup_event(
                start,
                "catalog_summary_load",
                format!(
                    "status=ready systems={} games={} elapsed_us={} path={}",
                    summary.systems.len(),
                    summary.total_game_count,
                    summary_t.elapsed().as_micros(),
                    summary_path.display()
                ),
            );
            Some(summary)
        }
        Ok(Some(_)) => {
            print_startup_event(
                start,
                "catalog_summary_load",
                format!(
                    "status=empty elapsed_us={} path={}",
                    summary_t.elapsed().as_micros(),
                    summary_path.display()
                ),
            );
            None
        }
        Ok(None) => {
            print_startup_event(
                start,
                "catalog_summary_load",
                format!(
                    "status=missing_or_stale elapsed_us={} path={}",
                    summary_t.elapsed().as_micros(),
                    summary_path.display()
                ),
            );
            None
        }
        Err(e) => {
            print_startup_event(
                start,
                "catalog_summary_load_failed",
                format!(
                    "elapsed_us={} path={} error={}",
                    summary_t.elapsed().as_micros(),
                    summary_path.display(),
                    e
                ),
            );
            None
        }
    }
}

fn sqlite_file_has_valid_header(path: &Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut header = [0u8; SQLITE_HEADER.len()];
    file.read_exact(&mut header).is_ok() && &header == SQLITE_HEADER
}

struct LaunchWorkerResult {
    result: Result<bool, launcher::LaunchError>,
    bench: Option<launcher::LaunchHandoffBenchResult>,
}

impl PendingLaunch {
    fn record_loading_frame(&mut self, loop_start: Instant) {
        self.loading_frames = self.loading_frames.saturating_add(1);
        if let Some(previous) = self.last_loop_start {
            let gap = loop_start.saturating_duration_since(previous).as_micros() as u64;
            self.max_frame_gap_us = self.max_frame_gap_us.max(gap);
        } else {
            let gap = loop_start
                .saturating_duration_since(self.loading_presented)
                .as_micros() as u64;
            self.max_frame_gap_us = self.max_frame_gap_us.max(gap);
        }
        self.last_loop_start = Some(loop_start);
    }
}

pub(super) fn recover_launcher_ui(f: &mut Fpga, ui: &UiDisplay, spawned_mister: &mut bool) {
    if *spawned_mister {
        launcher::stop_mister();
        let route = FpgaFramebufferRoute::for_ui(ui, FramebufferFormat::production_default());
        if let Err(e) = route.enable(f, ui.fb_w(), ui.fb_h()) {
            eprintln!("failed to recover Slint framebuffer route after launch failure: {e}");
        }
        *spawned_mister = false;
    }
}

pub(super) fn present_launcher_startup_frame(
    start: Instant,
    ui: &UiDisplay,
    disp: &mut Display,
    f: &mut Fpga,
    window: &Rc<MinimalSoftwareWindow>,
    target: &mut UiFrameTarget,
) {
    if launcher_return_reveal_enabled() {
        print_startup_event(
            start,
            "startup_splash_skipped",
            "reason=return_to_launcher reveal=first_launcher_frame",
        );
        return;
    }

    let settle_frames = launcher_reveal_settle_frames();
    let render_count = settle_frames.max(1);
    let mut render_us = 0u128;
    let mut vsync_hits = 0u32;
    let mut vsync_timeouts = 0u32;
    let mut vsync_errors = 0u32;

    for frame in 0..render_count {
        let draw_t = Instant::now();
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            let _ = target.render(renderer, ui);
        });
        render_us += draw_t.elapsed().as_micros();
        if frame < settle_frames {
            match disp.wait_vsync() {
                VsyncWaitStatus::Hit { .. } => vsync_hits += 1,
                VsyncWaitStatus::Timeout { .. } => vsync_timeouts += 1,
                VsyncWaitStatus::Error { .. } => vsync_errors += 1,
            }
        }
    }

    let copy_t = Instant::now();
    target.present_rows(f, disp, ui, 0, ui.render_h());
    print_startup_event(
        start,
        "startup_splash_presented",
        format!(
            "settle_frames={} render_count={} render_us={} copy_us={} vsync_hits={} vsync_timeouts={} vsync_errors={}",
            settle_frames,
            render_count,
            render_us,
            copy_t.elapsed().as_micros(),
            vsync_hits,
            vsync_timeouts,
            vsync_errors
        ),
    );
}

fn launcher_return_reveal_enabled() -> bool {
    matches!(
        std::env::var("MISTER_MAGIK_RETURN_TO_LAUNCHER")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn launch_handoff_bench_label() -> String {
    std::env::var("MISTER_LAUNCH_HANDOFF_LABEL").unwrap_or_else(|_| "launch-handoff".to_string())
}

fn launch_handoff_bench_trace_path() -> Option<String> {
    std::env::var("MISTER_LAUNCH_HANDOFF_TRACE")
        .ok()
        .filter(|path| !path.trim().is_empty())
}

fn launch_handoff_bench_delay() -> Duration {
    std::env::var("MISTER_LAUNCH_HANDOFF_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_LAUNCH_HANDOFF_BENCH_DELAY)
}

fn launch_handoff_bench_iterations() -> usize {
    std::env::var("MISTER_LAUNCH_HANDOFF_ITERATIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1)
}

#[allow(clippy::too_many_arguments)]
fn write_launch_handoff_bench_sample(
    trace_path: Option<&str>,
    label: &str,
    iteration: usize,
    action_start: Instant,
    loading_presented: Instant,
    max_frame_gap_us: u64,
    loading_frames_before_result: u64,
    handoff_result: Instant,
    recovery_presented: Instant,
    launch_prep_us: u64,
    handoff_wait_us: u64,
    result: &str,
) {
    let launch_action_to_loading_us = loading_presented
        .saturating_duration_since(action_start)
        .as_micros() as u64;
    let failure_recovery_us = recovery_presented
        .saturating_duration_since(handoff_result)
        .as_micros() as u64;
    let line = format!(
        "launch_handoff_sample\t{label}\t{iteration}\tlaunch_action_to_loading_us={launch_action_to_loading_us}\tmax_frame_gap_us={max_frame_gap_us}\tloading_frames_before_result={loading_frames_before_result}\tfailure_recovery_us={failure_recovery_us}\tlaunch_prep_us={launch_prep_us}\thandoff_wait_us={handoff_wait_us}\tresult={result}"
    );
    println!("{line}");
    if let Some(path) = trace_path {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

fn launcher_reveal_settle_frames() -> u32 {
    std::env::var("MISTER_LAUNCHER_REVEAL_SETTLE_FRAMES")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(DEFAULT_LAUNCHER_REVEAL_SETTLE_FRAMES)
        .min(MAX_LAUNCHER_REVEAL_SETTLE_FRAMES)
}

pub(super) fn run_launcher_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    f: &mut Fpga,
    window: &Rc<MinimalSoftwareWindow>,
    target: &mut UiFrameTarget,
    mut pad: PadPool,
    app: slint_ui::launcher::Launcher,
    animation_clock: &AnimationClock,
) {
    let start = Instant::now();
    let mut frames = 0u64;
    let launcher_bench_scenario = LauncherBenchScenario::from_env();
    let launcher_bench_launch_handoff =
        launcher_bench_scenario == Some(LauncherBenchScenario::LaunchHandoff);
    let launch_handoff_bench_label = launch_handoff_bench_label();
    let launch_handoff_bench_trace = launch_handoff_bench_trace_path();
    let launch_handoff_bench_delay = launch_handoff_bench_delay();
    let launch_handoff_bench_iterations = launch_handoff_bench_iterations();
    let mut launch_handoff_bench_iteration = 0usize;
    let mut pending_launch: Option<PendingLaunch> = None;
    let bench_starts_on_arcade =
        launcher_bench_scenario.is_some_and(|scenario| scenario.starts_on_arcade());
    let benchmark_media_interaction_active = launcher_bench_scenario.is_some();
    let env_start_screen = launcher_start_screen_from_env();
    let start_screen = env_start_screen
        .or_else(|| bench_starts_on_arcade.then_some(Screen::Arcade))
        .unwrap_or(Screen::Home);
    let lock_screen = launcher_lock_screen_from_env()
        .or_else(|| bench_starts_on_arcade.then_some(Screen::Arcade));
    let launch_return_restore_allowed =
        env_start_screen.is_none() && launcher_bench_scenario.is_none() && lock_screen.is_none();
    let mut pending_launch_return_state =
        launcher::take_launch_return_state().filter(|_| launch_return_restore_allowed);
    let arcade_catalog_required_at_start =
        start_screen == Screen::Arcade || lock_screen == Some(Screen::Arcade);
    let mut nav = LauncherNav::new();
    nav.screen = start_screen;
    let mut setup = SetupNav::new();
    let mut loading_title = String::new();
    let mut launch_started = Instant::now();
    let mut launch_spawned_mister = false;
    let mut last_clock_update = Instant::now() - Duration::from_secs(2);
    let mut last_clock_text = launcher_clock_text();
    let mut launcher_bench_next_step: Instant;
    let mut launcher_bench_step_idx = 0usize;
    let auto_launch_selected = launcher_auto_launch_selected_enabled();
    let mut auto_launch_selected_done = false;
    let dirty_opt = launcher_dirty_opt_enabled();
    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!(
        "launcher running {label} — {} pad(s), D-pad to move, A to select, Home to go back...",
        pad.len()
    );
    println!(
        "launcher_mode={} fb_format={}",
        "launcher",
        FramebufferFormat::production_default().label()
    );
    if let Some(scenario) = launcher_bench_scenario {
        println!("launcher_bench_scenario={}", scenario.label());
    }
    println!(
        "launcher_start_screen={} launcher_lock_screen={}",
        screen_label(start_screen),
        lock_screen.map(screen_label).unwrap_or("none")
    );
    println!(
        "launcher_dirty_opt={}",
        if dirty_opt { "on" } else { "off" }
    );
    boot_analytics::event(
        "launcher_loop_start",
        format!("label={label} pads={}", pad.len()),
    );
    if AUTO_CONTROLLER_SETUP_ENABLED {
        if let Some(idx) = pad.index_needing_setup() {
            let status = pad.db().registry_status(pad.info_at(idx));
            eprintln!("controller setup: pad {idx} needs setup ({status:?}) - showing prompt");
            setup.open_for(status, idx);
        }
    }
    let mut pacer = VsyncPacer::from_env();
    let mut present_probe = PresentProbe::from_env();
    if launcher_bench_scenario.is_some() {
        let warm_t = Instant::now();
        match preview_worker::warm_preview_archives_from_env() {
            Ok(loaded) => print_startup_event(
                start,
                "preview_archive_warm",
                format!(
                    "loaded={} elapsed_us={}",
                    if loaded { 1 } else { 0 },
                    warm_t.elapsed().as_micros()
                ),
            ),
            Err(e) => {
                eprintln!("preview archive warm failed before launcher benchmark: {e}");
                print_startup_event(start, "preview_archive_warm_failed", e);
                std::process::exit(13);
            }
        }
    }
    let mut preview = PreviewState::new();
    let mut launcher_bench_waiting_for_initial_preview =
        launcher_bench_scenario.is_some_and(|scenario| scenario.starts_on_arcade());
    let mut route_guard = FramebufferRouteGuard::from_env();
    let mut preview_transition = PreviewTransitionDemo::from_env();
    let mut effect_label_overlay = preview_transition
        .label_overlay_enabled()
        .then(EffectLabelOverlay::new);
    let transition_picker_enabled = preview_transition.picker_enabled();
    let mut transition_picker_prev_left = false;
    let mut transition_picker_prev_right = false;
    let mut arcade_list_renderer = ArcadeListRenderer::new();
    let cpu = cpu_profile::start();
    let mut bridge_models = LauncherBridgeModels::default();
    let mut catalog_version = 0usize;
    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    println!(
        "preview_visual_pct={} preview_blitter=raw",
        preview_visual_pct()
    );
    println!(
        "preview_transition={} segment_secs={} duration_ms={}",
        preview_transition.labels(),
        preview_transition.segment.as_secs(),
        preview_transition.duration.as_millis()
    );
    let mut catalog = empty_arcade_catalog(&arcade_root);
    let mut catalog_ready = false;
    let catalog_refresh_policy = catalog_refresh_policy();
    let catalog_refresh = catalog_refresh_policy.force_requested();
    let catalog_worker_enabled = catalog_refresh_policy.worker_enabled();
    let deferred_library_rebuild = consume_library_rebuild_marker(catalog_worker_enabled, start);
    let mut catalog_session = LauncherCatalogSession::new(deferred_library_rebuild);
    let mut catalog_rx = None;
    let mut media_session = ScreenshotMediaUpdateSession::default();
    let mut media_handle = None;
    let mut media_worker_unavailable = false;
    let mut library_changed_dialog_test = LibraryChangedDialogTestDriver::from_env(start);
    let sqlite_path = library_db::default_sqlite_path();
    let summary_path = catalog_summary::summary_path_for_sqlite(&sqlite_path);
    let summary_seed = read_catalog_summary_seed(&sqlite_path, &summary_path, start);
    if let Some(summary) = summary_seed {
        catalog = catalog_from_summary(&arcade_root, &summary);
        catalog_ready = true;
        catalog_session.note_summary_seed_ready();
        media_session.request_catalog_seed();
        catalog_version = catalog_version.wrapping_add(1);
        let request = if deferred_library_rebuild {
            CatalogWorkerRequest::ForceBuild
        } else {
            ready_catalog_worker_request(catalog_refresh_policy)
        };
        if catalog_worker_enabled {
            print_startup_event(start, "catalog_worker_start", &arcade_root);
            catalog_rx = Some(start_library_catalog_worker(
                arcade_root.clone(),
                request,
                CatalogWorkerInitialCache::ProbeSqlite,
            ));
        } else {
            print_startup_event(
                start,
                "catalog_refresh_decision",
                format!(
                    "cache_state=summary refresh_policy={} background_validation=false plan=load_only",
                    catalog_refresh_policy.label()
                ),
            );
            catalog_session.mark_refresh_done();
        }
    } else {
        match library_db::load_arcade_catalog_from_sqlite(&arcade_root) {
            Ok(loaded) if !loaded.catalog.games.is_empty() => {
                print_startup_event(
                    start,
                    "catalog_cache_load_sync",
                    catalog_load_timing_detail(&loaded),
                );
                catalog = loaded.catalog;
                catalog_ready = true;
                catalog_session.note_cached_catalog_ready();
                media_session.request_catalog_seed();
                catalog_version = catalog_version.wrapping_add(1);
                apply_forced_arcade_selected(&mut nav, &catalog);
                apply_pending_launch_return_state(
                    &mut nav,
                    &catalog,
                    &mut pending_launch_return_state,
                );
                let request = ready_catalog_worker_request(catalog_refresh_policy);
                if deferred_library_rebuild {
                    print_startup_event(start, "catalog_worker_start", &arcade_root);
                    catalog_rx = Some(start_library_catalog_worker(
                        arcade_root.clone(),
                        CatalogWorkerRequest::ForceBuild,
                        CatalogWorkerInitialCache::AlreadyLoadedReady,
                    ));
                } else if request != CatalogWorkerRequest::LoadOnly {
                    let delay = catalog_background_validation_delay();
                    print_startup_event(
                        start,
                        "catalog_worker_deferred",
                        format!(
                            "root={} request={} delay_ms={}",
                            arcade_root,
                            request.label(),
                            delay.as_millis()
                        ),
                    );
                    catalog_session.defer_catalog_worker(arcade_root.clone(), request);
                } else {
                    print_startup_event(
                        start,
                        "catalog_refresh_decision",
                        format!(
                            "cache_state=ready refresh_policy={} background_validation=false plan=load_only",
                            catalog_refresh_policy.label()
                        ),
                    );
                    catalog_rx = None;
                    catalog_session.mark_refresh_done();
                }
            }
            Ok(loaded) => {
                print_startup_event(
                    start,
                    "catalog_cache_empty",
                    catalog_load_timing_detail(&loaded),
                );
                if catalog_worker_enabled {
                    print_startup_event(start, "catalog_worker_start", &arcade_root);
                    catalog_rx = Some(start_library_catalog_worker(
                        arcade_root.clone(),
                        CatalogWorkerRequest::ForceBuild,
                        CatalogWorkerInitialCache::ProbeSqlite,
                    ));
                } else {
                    print_startup_event(
                        start,
                        "catalog_refresh_decision",
                        format!(
                            "cache_state=empty refresh_policy={} background_validation=false plan=load_only",
                            catalog_refresh_policy.label()
                        ),
                    );
                    catalog_session.mark_refresh_done();
                }
            }
            Err(e) => {
                eprintln!("arcade catalog cache load failed: {e}");
                print_startup_event(start, "catalog_cache_load_failed", e);
                if catalog_worker_enabled {
                    print_startup_event(start, "catalog_worker_start", &arcade_root);
                    catalog_rx = Some(start_library_catalog_worker(
                        arcade_root.clone(),
                        CatalogWorkerRequest::ForceBuild,
                        CatalogWorkerInitialCache::ProbeSqlite,
                    ));
                } else {
                    print_startup_event(
                        start,
                        "catalog_refresh_decision",
                        format!(
                            "cache_state=missing refresh_policy={} background_validation=false plan=load_only",
                            catalog_refresh_policy.label()
                        ),
                    );
                    catalog_session.mark_refresh_done();
                }
            }
        }
    }
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let bridge_systems_t = Instant::now();
    bridge.set_game_systems(bridge_models.game_systems(&catalog, catalog_version));
    print_startup_event(
        start,
        "catalog_bridge_systems",
        format!(
            "catalog_ready={} systems={} elapsed_us={}",
            catalog_ready,
            catalog.systems.len(),
            bridge_systems_t.elapsed().as_micros()
        ),
    );
    let catalog_scan_title = if catalog_ready {
        if catalog_session.foreground_update() {
            "Indexing library".to_string()
        } else if catalog_refresh {
            "Validating library".to_string()
        } else {
            String::new()
        }
    } else if !catalog_worker_enabled {
        String::new()
    } else {
        "Indexing library".to_string()
    };
    let catalog_scan_detail = if catalog_ready {
        if catalog_session.foreground_update() {
            "Rebuilding catalog with latest games...".to_string()
        } else {
            format!("Using cached {} games", catalog.len())
        }
    } else if !catalog_worker_enabled {
        "Catalog worker disabled for benchmark restart".to_string()
    } else {
        "No cached catalog; scanning library...".to_string()
    };
    LauncherStatusPresenter::new(&bridge).sync_catalog_scan(CatalogScanBridgeStatus::new(
        initial_catalog_scan_visible(
            catalog_ready,
            arcade_catalog_required_at_start,
            catalog_worker_enabled,
            catalog_session.foreground_update(),
        ),
        false,
        catalog_scan_message(catalog_session.foreground_update()),
        catalog_scan_title,
        catalog_scan_detail,
        -1,
    ));
    let bridge_sync_t = Instant::now();
    sync_bridge_launcher(
        &app,
        &pad,
        &nav,
        &setup,
        "",
        "",
        Some(&catalog),
        &mut preview,
        &mut bridge_models,
        catalog_version,
        false,
    );
    print_startup_event(
        start,
        "catalog_bridge_sync",
        format!(
            "catalog_ready={} games={} elapsed_us={}",
            catalog_ready,
            catalog.len(),
            bridge_sync_t.elapsed().as_micros()
        ),
    );
    window.request_redraw();
    let run_start = if arcade_catalog_required_at_start && catalog_ready {
        Instant::now()
    } else {
        start
    };
    launcher_bench_next_step = run_start;
    let preview_scroll_exit_at = preview_scroll_exit_after_trace_deadline(run_start);
    let mut first_render_logged = false;
    let mut first_vsync_logged = false;
    let mut frame_accounting = LauncherFrameAccounting::new(run_start);
    let mut catalog_scan_redraw = CatalogScanRedraw::new();
    let mut route_reassert_count = 0u64;
    let mut last_route_reassert_frame = 0u64;
    let mut last_route_reassert_ok = false;
    let mut last_route_reassert_error = String::new();
    while (secs == 0 || run_start.elapsed().as_secs() < secs)
        && preview_scroll_exit_at.is_none_or(|deadline| Instant::now() < deadline)
    {
        let loop_start = Instant::now();
        let prepare_trace_enabled = frame_accounting.preview_scroll_trace_enabled();
        let mut prepare_trace = LauncherPrepareTrace::default();
        if let Some(pending) = pending_launch.as_mut() {
            pending.record_loading_frame(loop_start);
        }
        let launching = launcher::launch_in_progress() || !loading_title.is_empty();
        let setup_active = setup.is_active();
        let mut light_bridge_dirty = false;
        let mut full_bridge_dirty = false;
        apply_screenshot_media_update_effects(
            media_session.clear_progress_if_due(loop_start),
            &app,
            &catalog,
            &mut media_handle,
            &mut media_worker_unavailable,
            &mut full_bridge_dirty,
            start,
        );
        let mut route_action = FramebufferRouteAction {
            reassert_route: false,
            force_full_present: false,
        };
        let defer_selected_preview = false;
        let mut preview_scheduled_this_loop = false;
        if !launching {
            route_action = route_guard.tick(frames);
            if route_action.reassert_route {
                let route =
                    FpgaFramebufferRoute::for_ui(ui, FramebufferFormat::production_default());
                match route.enable(f, ui.fb_w(), ui.fb_h()) {
                    Ok(flag) => {
                        route_reassert_count = route_reassert_count.saturating_add(1);
                        last_route_reassert_frame = frames;
                        last_route_reassert_ok = true;
                        last_route_reassert_error.clear();
                        boot_analytics::event(
                            "launcher_fb_route_reasserted",
                            format!("frame={frames} support_flag={flag}"),
                        );
                    }
                    Err(e) => {
                        eprintln!("failed to reassert Slint framebuffer route: {e}");
                        route_action.force_full_present = false;
                        route_reassert_count = route_reassert_count.saturating_add(1);
                        last_route_reassert_frame = frames;
                        last_route_reassert_ok = false;
                        last_route_reassert_error = e.to_string();
                        boot_analytics::event(
                            "launcher_fb_route_reassert_failed",
                            format!("frame={frames} error={e}"),
                        );
                    }
                }
            }
        }
        if last_clock_update.elapsed() >= Duration::from_secs(1) {
            let clock_text = launcher_clock_text();
            if dirty_opt {
                if clock_text != last_clock_text {
                    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                    bridge.set_clock_text(clock_text.clone().into());
                    last_clock_text = clock_text;
                    light_bridge_dirty = true;
                }
            } else {
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                bridge.set_clock_text(clock_text.clone().into());
                last_clock_text = clock_text;
                full_bridge_dirty = true;
            }
            last_clock_update = Instant::now();
        }

        let catalog_worker_trace_start = prepare_trace_enabled.then(Instant::now);
        if let Some(worker) = catalog_session.maybe_start_deferred_worker(
            catalog_rx.is_some(),
            frame_accounting.first_visible_copy_done(),
            loop_start,
            catalog_background_validation_delay(),
        ) {
            print_startup_event(start, "catalog_worker_start", &worker.root);
            catalog_rx = Some(start_library_catalog_worker(
                worker.root,
                worker.request,
                worker.initial_cache,
            ));
        }

        if !catalog_session.refresh_done() {
            while let Some(message) = catalog_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                let media_gate =
                    if matches!(&message, CatalogWorkerMessage::SystemDiscovered { .. }) {
                        let media_gate = media_session.current_gate(
                            frame_accounting.first_visible_copy_done(),
                            pending_launch.is_some() || launching,
                            benchmark_media_interaction_active,
                            loop_start,
                        );
                        apply_screenshot_media_update_effects(
                            media_session.sync_gate(media_gate),
                            &app,
                            &catalog,
                            &mut media_handle,
                            &mut media_worker_unavailable,
                            &mut full_bridge_dirty,
                            start,
                        );
                        Some(media_gate)
                    } else {
                        None
                    };
                let effects = catalog_session.handle_worker_message(
                    CatalogWorkerMessageContext {
                        catalog_ready,
                        screen: nav.screen,
                        media_gate,
                    },
                    message,
                    loop_start,
                );
                apply_catalog_session_effects(
                    effects,
                    &app,
                    &pad,
                    &mut nav,
                    &setup,
                    &loading_title,
                    &mut catalog,
                    &mut catalog_ready,
                    &mut catalog_version,
                    &mut pending_launch_return_state,
                    &mut preview,
                    &mut bridge_models,
                    &mut media_session,
                    &mut media_handle,
                    &mut media_worker_unavailable,
                    &mut catalog_rx,
                    &mut full_bridge_dirty,
                    start,
                );
            }
        }
        if let Some(trace_start) = catalog_worker_trace_start {
            prepare_trace.catalog_worker_us = trace_start.elapsed().as_micros();
        }

        let media_worker_trace_start = prepare_trace_enabled.then(Instant::now);
        while let Some(message) = media_handle.as_ref().and_then(|handle| handle.try_recv()) {
            let bridge = app.global::<slint_ui::launcher::MisterBridge>();
            let catalog_scan_visible = bridge.get_catalog_scan_visible();
            let effects =
                media_session.handle_worker_message(message, catalog_scan_visible, loop_start);
            apply_screenshot_media_update_effects(
                effects,
                &app,
                &catalog,
                &mut media_handle,
                &mut media_worker_unavailable,
                &mut full_bridge_dirty,
                start,
            );
        }
        if let Some(trace_start) = media_worker_trace_start {
            prepare_trace.media_worker_us = trace_start.elapsed().as_micros();
        }

        if let Some(worker_result) = pending_launch
            .as_ref()
            .and_then(|pending| pending.rx.try_recv().ok())
        {
            let pending = pending_launch.take().expect("pending launch result");
            let result_received = Instant::now();
            match worker_result.result {
                Ok(spawned) => {
                    launch_started = Instant::now();
                    launch_spawned_mister = spawned;
                }
                Err(e) => {
                    launch_started = Instant::now();
                    if worker_result.bench.is_none() {
                        launcher::remove_launch_return_state();
                    }
                    launch_spawned_mister |= e.spawned_mister();
                    loading_title.clear();
                    launcher::reset_launch();
                    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                    LauncherStatusPresenter::new(&bridge)
                        .sync_loading("Launch failed", "Returning to launcher...");
                    recover_launcher_ui(f, ui, &mut launch_spawned_mister);
                    update_slint_animations(animation_clock);
                    let mut recovery_rect = None;
                    window.draw_if_needed(|renderer| {
                        let region = target.render(renderer, ui);
                        recovery_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
                    });
                    if let Some(rect) = recovery_rect {
                        let _ = target.present_rect(f, disp, ui, rect);
                    } else {
                        target.present_rows(f, disp, ui, 0, ui.render_h());
                    }
                    let recovery_presented = Instant::now();
                    window.request_redraw();
                    if let (Some(bench), Some(iteration)) =
                        (worker_result.bench.as_ref(), pending.bench_iteration)
                    {
                        write_launch_handoff_bench_sample(
                            launch_handoff_bench_trace.as_deref(),
                            &launch_handoff_bench_label,
                            iteration,
                            pending.action_start,
                            pending.loading_presented,
                            pending.max_frame_gap_us,
                            pending.loading_frames.max(1),
                            result_received,
                            recovery_presented,
                            bench.prepare_us,
                            bench.handoff_us,
                            "error",
                        );
                    }
                    LauncherStatusPresenter::new(&bridge).sync_loading("", "");
                    eprintln!("game launch failed: {e}");
                }
            }
        }

        if let Some(scenario) = launcher_bench_scenario {
            if catalog_ready && launcher_bench_waiting_for_initial_preview {
                let cache_state = preview.trace_cache_state();
                if launcher_bench_initial_preview_ready(scenario, cache_state) {
                    launcher_bench_waiting_for_initial_preview = false;
                    launcher_bench_next_step = Instant::now();
                    print_startup_event(
                        start,
                        "launcher_bench_preview_ready",
                        format!("cache_state={cache_state}"),
                    );
                }
            }
            if catalog_ready
                && !launcher_bench_waiting_for_initial_preview
                && launcher_bench_next_step.elapsed() >= scenario.period()
            {
                let before = LauncherBridgeKey::from_nav(&nav);
                let bench_step_ran = launcher_bench_step(
                    scenario,
                    &mut nav,
                    &catalog,
                    None,
                    launcher_bench_step_idx,
                    Instant::now(),
                );
                if bench_step_ran {
                    let after = LauncherBridgeKey::from_nav(&nav);
                    if before != after {
                        media_session.note_nav_change(&before, &after, Instant::now());
                        if !dirty_opt || before.screen != after.screen {
                            full_bridge_dirty = true;
                        } else {
                            light_bridge_dirty = true;
                        }
                    }
                }
                launcher_bench_step_idx =
                    launcher_bench_next_step_index(launcher_bench_step_idx, bench_step_ran);
                launcher_bench_next_step = Instant::now();
            }
        }

        if let Some(screen) = lock_screen {
            nav.screen = screen;
        }

        if !launching {
            let pad_changed = pad.poll_with_debug_labels(setup_active);
            let frame_now = Instant::now();

            if setup_active && setup.target_pad_idx >= pad.len() {
                eprintln!(
                    "controller setup: pad {} disappeared; closing setup flow",
                    setup.target_pad_idx
                );
                setup.advance_to_next_pad(&pad);
                full_bridge_dirty = true;
            }

            let input_session = ControllerSetupInputSession::new(&pad, &setup);
            let launcher_state = input_session.launcher_state().clone();
            let setup_state = input_session.setup_state().clone();
            let active_idx = pad.active_idx();
            let info = pad.info();

            if launcher_bench_scenario.is_none() && setup.is_active() {
                let setup_before = SetupBridgeKey::from_setup(&setup);
                let setup_info = pad.info_at(setup.target_pad_idx);
                match setup.handle_input(&setup_state, frame_now, setup_info, pad.db()) {
                    SetupAction::None => {}
                    SetupAction::RegisterNew => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.register_new_at(idx) {
                            eprintln!("controller setup: register new: {e}");
                        }
                    }
                    SetupAction::ClaimExisting { list_index } => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.claim_existing_at(idx, list_index) {
                            eprintln!("controller setup: claim existing: {e}");
                        }
                    }
                    SetupAction::SaveFinish { label, kind } => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.finish_setup_at(idx, label, kind) {
                            eprintln!("controller setup: save: {e}");
                        } else {
                            eprintln!(
                                "controller setup: saved \"{}\" ({})",
                                pad.db().display_label(pad.info_at(idx)),
                                kind.as_str()
                            );
                        }
                        setup.advance_to_next_pad(&pad);
                    }
                    SetupAction::Done => {
                        setup.advance_to_next_pad(&pad);
                    }
                }
                let setup_after = SetupBridgeKey::from_setup(&setup);
                full_bridge_dirty |= pad_changed || setup_before != setup_after;
            } else if launcher_bench_scenario.is_none() || launcher_bench_launch_handoff {
                if AUTO_CONTROLLER_SETUP_ENABLED && pad_changed {
                    let setup_before = SetupBridgeKey::from_setup(&setup);
                    setup.maybe_open(info, active_idx, pad.db(), true);
                    full_bridge_dirty |= setup_before != SetupBridgeKey::from_setup(&setup);
                }
                if !setup.is_active() {
                    let nav_before = LauncherBridgeKey::from_nav(&nav);
                    if transition_picker_enabled && nav.screen == Screen::Arcade {
                        let left = launcher_state.dpad_left && !transition_picker_prev_left;
                        let right = launcher_state.dpad_right && !transition_picker_prev_right;
                        let changed = if left {
                            preview_transition.cycle_picker(-1)
                        } else if right {
                            preview_transition.cycle_picker(1)
                        } else {
                            false
                        };
                        if changed {
                            println!(
                                "preview_transition_picker={}",
                                preview_transition
                                    .current_label(frame_now.duration_since(run_start))
                            );
                            window.request_redraw();
                        }
                    }
                    transition_picker_prev_left = launcher_state.dpad_left;
                    transition_picker_prev_right = launcher_state.dpad_right;
                    let mut nav_state = launcher_state.clone();
                    if let Some(test_state) =
                        library_changed_dialog_test.input_for(&nav, loop_start, start)
                    {
                        nav_state = test_state;
                    }
                    let event = if launcher_bench_launch_handoff
                        && launch_handoff_bench_iteration < launch_handoff_bench_iterations
                        && catalog_ready
                        && !launcher_bench_waiting_for_initial_preview
                        && nav.screen == Screen::Arcade
                    {
                        let event = active_system(&catalog, &nav)
                            .and_then(|system| {
                                catalog.system_game_at(&system.id, nav.arcade.selected)
                            })
                            .map(|game| launcher::LauncherEvent {
                                action: LauncherAction::LaunchGame,
                                path: Some(game.mra_path.to_string()),
                            });
                        if event.is_some() {
                            launch_handoff_bench_iteration += 1;
                        }
                        event
                    } else if auto_launch_selected
                        && !auto_launch_selected_done
                        && catalog_ready
                        && nav.screen == Screen::Arcade
                    {
                        auto_launch_selected_done = true;
                        active_system(&catalog, &nav)
                            .and_then(|system| {
                                catalog.system_game_at(&system.id, nav.arcade.selected)
                            })
                            .map(|game| launcher::LauncherEvent {
                                action: LauncherAction::LaunchGame,
                                path: Some(game.mra_path.to_string()),
                            })
                    } else if launcher_bench_launch_handoff {
                        None
                    } else {
                        nav.handle_input(&nav_state, frame_now, &catalog)
                    };
                    if let Some(event) = event {
                        match event.action {
                            LauncherAction::ExitToMister => {
                                loading_title = "Exit to MiSTer".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Return to MiSTer MagiK after reboot",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                    false,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, ui);
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                match launcher::exit_to_mister() {
                                    Ok(()) => std::process::exit(0),
                                    Err(e) => {
                                        eprintln!("exit to MiSTer failed: {e}");
                                        loading_title.clear();
                                    }
                                }
                            }
                            LauncherAction::ResetDatabase => {
                                loading_title = "Shutting down…".to_string();
                                apply_screenshot_media_update_effects(
                                    media_session.shutdown_for_reset(),
                                    &app,
                                    &catalog,
                                    &mut media_handle,
                                    &mut media_worker_unavailable,
                                    &mut full_bridge_dirty,
                                    start,
                                );
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Restarting MiSTer",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                    false,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, ui);
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                std::thread::sleep(Duration::from_millis(250));
                                match launcher::reset_database_and_reboot() {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        eprintln!("reset database failed: {e}");
                                        loading_title.clear();
                                    }
                                }
                            }
                            LauncherAction::Restart => {
                                loading_title = "Shutting down…".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Restarting MiSTer",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                    false,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, ui);
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                std::thread::sleep(Duration::from_millis(250));
                                match launcher::reboot_mister() {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        eprintln!("restart failed: {e}");
                                        loading_title.clear();
                                    }
                                }
                            }
                            LauncherAction::ContinueWithStaleLibrary => {
                                let effects = catalog_session.continue_with_stale_library();
                                apply_catalog_session_effects(
                                    effects,
                                    &app,
                                    &pad,
                                    &mut nav,
                                    &setup,
                                    &loading_title,
                                    &mut catalog,
                                    &mut catalog_ready,
                                    &mut catalog_version,
                                    &mut pending_launch_return_state,
                                    &mut preview,
                                    &mut bridge_models,
                                    &mut media_session,
                                    &mut media_handle,
                                    &mut media_worker_unavailable,
                                    &mut catalog_rx,
                                    &mut full_bridge_dirty,
                                    start,
                                );
                                window.request_redraw();
                                continue;
                            }
                            LauncherAction::RebuildLibrary => {
                                let effects = catalog_session.rebuild_library(arcade_root.clone());
                                apply_catalog_session_effects(
                                    effects,
                                    &app,
                                    &pad,
                                    &mut nav,
                                    &setup,
                                    &loading_title,
                                    &mut catalog,
                                    &mut catalog_ready,
                                    &mut catalog_version,
                                    &mut pending_launch_return_state,
                                    &mut preview,
                                    &mut bridge_models,
                                    &mut media_session,
                                    &mut media_handle,
                                    &mut media_worker_unavailable,
                                    &mut catalog_rx,
                                    &mut full_bridge_dirty,
                                    start,
                                );
                                window.request_redraw();
                                continue;
                            }
                            LauncherAction::LaunchGame => {}
                        }
                        let Some(mra) = event.path else {
                            continue;
                        };
                        let launch_action_start = Instant::now();
                        loading_title =
                            format!("Loading {}…", launcher::game_title(&catalog, &mra));
                        sync_bridge_launcher(
                            &app,
                            &pad,
                            &nav,
                            &setup,
                            &loading_title,
                            "",
                            Some(&catalog),
                            &mut preview,
                            &mut bridge_models,
                            catalog_version,
                            false,
                        );
                        window.request_redraw();
                        update_slint_animations(animation_clock);
                        window.draw_if_needed(|renderer| {
                            let region = target.render(renderer, ui);
                            let _ = region;
                        });
                        let _pace = pacer.wait();
                        target.present_rows(f, disp, ui, 0, ui.render_h());
                        let loading_presented = Instant::now();
                        if !launcher_bench_launch_handoff {
                            if let Some(state) =
                                launcher::capture_launch_return_state(&nav, &catalog, &mra)
                            {
                                if let Err(e) = launcher::save_launch_return_state(&state) {
                                    eprintln!("failed to save launch return state: {e}");
                                }
                            }
                        }
                        let (tx, rx) = mpsc::channel();
                        let worker_mra = mra.clone();
                        let bench_delay = launch_handoff_bench_delay;
                        let bench_iteration =
                            launcher_bench_launch_handoff.then_some(launch_handoff_bench_iteration);
                        thread::Builder::new()
                            .name("launch-handoff".to_string())
                            .spawn(move || {
                                let result = if bench_iteration.is_some() {
                                    let bench = launcher::execute_game_launch_handoff_bench(
                                        &worker_mra,
                                        bench_delay,
                                    );
                                    LaunchWorkerResult {
                                        result: bench.result.clone(),
                                        bench: Some(bench),
                                    }
                                } else {
                                    LaunchWorkerResult {
                                        result: launcher::execute_game_launch(&worker_mra),
                                        bench: None,
                                    }
                                };
                                let _ = tx.send(result);
                            })
                            .expect("spawn launch-handoff");
                        pending_launch = Some(PendingLaunch {
                            rx,
                            action_start: launch_action_start,
                            loading_presented,
                            bench_iteration,
                            loading_frames: 1,
                            max_frame_gap_us: 0,
                            last_loop_start: None,
                        });
                        window.request_redraw();
                    }
                    let nav_after = LauncherBridgeKey::from_nav(&nav);
                    if nav_before != nav_after {
                        media_session.note_nav_change(&nav_before, &nav_after, Instant::now());
                    }
                    if pad_changed && nav.screen == Screen::Controller {
                        full_bridge_dirty = true;
                    } else if pad_changed && !dirty_opt {
                        full_bridge_dirty = true;
                    }
                    if nav_before != nav_after {
                        if !dirty_opt || nav_before.screen != nav_after.screen {
                            full_bridge_dirty = true;
                        } else {
                            light_bridge_dirty = true;
                        }
                    }
                }
            }

            if let Some(screen) = lock_screen {
                nav.screen = screen;
            }

            if full_bridge_dirty {
                sync_bridge_launcher(
                    &app,
                    &pad,
                    &nav,
                    &setup,
                    &loading_title,
                    "",
                    Some(&catalog),
                    &mut preview,
                    &mut bridge_models,
                    catalog_version,
                    defer_selected_preview,
                );
                preview_scheduled_this_loop = nav.screen == Screen::Arcade;
                window.request_redraw();
            } else if light_bridge_dirty {
                let active_games = if nav.screen == Screen::Arcade {
                    Some(active_system_game_slice(&catalog, &nav))
                } else {
                    None
                };
                sync_bridge_launcher_light(
                    &app,
                    &nav,
                    &setup,
                    &loading_title,
                    "",
                    &catalog,
                    active_games,
                    &mut preview,
                    defer_selected_preview,
                );
                preview_scheduled_this_loop = nav.screen == Screen::Arcade;
                window.request_redraw();
            }
        } else {
            let _ = pad.poll();
            if pending_launch.is_none()
                && launcher::mister_running_arcade_core()
                && launch_started.elapsed() > Duration::from_millis(500)
            {
                println!("arcade core running — handing off to MiSTer");
                std::process::exit(0);
            } else if pending_launch.is_none() && launch_started.elapsed() > Duration::from_secs(90)
            {
                eprintln!("game launch timed out");
                recover_launcher_ui(f, ui, &mut launch_spawned_mister);
                std::process::exit(1);
            }
        }

        let media_gate_trace_start = prepare_trace_enabled.then(Instant::now);
        {
            let media_gate = media_session.current_gate(
                frame_accounting.first_visible_copy_done(),
                pending_launch.is_some() || launching,
                benchmark_media_interaction_active,
                loop_start,
            );
            apply_screenshot_media_update_effects(
                media_session.sync_gate(media_gate),
                &app,
                &catalog,
                &mut media_handle,
                &mut media_worker_unavailable,
                &mut full_bridge_dirty,
                start,
            );
            apply_screenshot_media_update_effects(
                media_session.apply_gate(media_gate),
                &app,
                &catalog,
                &mut media_handle,
                &mut media_worker_unavailable,
                &mut full_bridge_dirty,
                start,
            );
            apply_screenshot_media_update_effects(
                media_session.sync_gate(media_gate),
                &app,
                &catalog,
                &mut media_handle,
                &mut media_worker_unavailable,
                &mut full_bridge_dirty,
                start,
            );
        }
        if let Some(trace_start) = media_gate_trace_start {
            prepare_trace.media_gate_us = trace_start.elapsed().as_micros();
        }

        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
        let catalog_scan_visible = bridge.get_catalog_scan_visible();
        let catalog_scan_percent = bridge.get_catalog_scan_percent();
        let catalog_background_scan_visible = bridge.get_catalog_background_scan_visible();
        let confirm_visible = bridge.get_confirm_visible();
        let confirm_selected = bridge.get_confirm_selected();
        let status_write_due = frame_accounting.status_write_due();
        let status_string_copy_start = (status_write_due
            && frame_accounting.preview_scroll_trace_enabled())
        .then(Instant::now);
        let status_text =
            status_write_due.then(|| LauncherStatusTextSnapshot::from_bridge(&bridge));
        let status_string_copy_us = status_string_copy_start
            .map(|start| start.elapsed().as_micros())
            .unwrap_or(0);
        prepare_trace.status_string_copy_us = status_string_copy_us;
        let status_string_copy_bytes = status_text
            .as_ref()
            .map(LauncherStatusTextSnapshot::bytes_len)
            .unwrap_or(0);
        let games_found_detail_changed = if catalog_scan_visible && catalog_scan_percent < 0 {
            catalog_session
                .tick_games_found_counter(loop_start)
                .is_some_and(|detail| {
                    LauncherStatusPresenter::new(&bridge).sync_catalog_scan_detail(detail);
                    true
                })
        } else {
            false
        };
        if launching
            || games_found_detail_changed
            || catalog_scan_redraw.should_request(
                catalog_scan_visible,
                catalog_background_scan_visible,
                catalog_scan_percent,
                loop_start,
            )
        {
            window.request_redraw();
        }
        let active_arcade_games = if !launching && nav.screen == Screen::Arcade {
            active_system_game_slice(&catalog, &nav)
        } else {
            &[]
        };
        let preview_schedule_trace_start = prepare_trace_enabled.then(Instant::now);
        if dirty_opt && !preview_scheduled_this_loop && !launching && nav.screen == Screen::Arcade {
            let bridge = app.global::<slint_ui::launcher::MisterBridge>();
            if schedule_arcade_preview_window(
                &bridge,
                active_arcade_games,
                nav.arcade.selected,
                &mut preview,
                defer_selected_preview,
            ) {
                window.request_redraw();
            }
        }
        if let Some(trace_start) = preview_schedule_trace_start {
            prepare_trace.preview_schedule_us = trace_start.elapsed().as_micros();
        }
        let preview_apply_trace_start = prepare_trace_enabled.then(Instant::now);
        if !launching && apply_ready_preview(&app, &mut preview, defer_selected_preview) {
            window.request_redraw();
        }
        if let Some(trace_start) = preview_apply_trace_start {
            prepare_trace.preview_apply_us = trace_start.elapsed().as_micros();
        }

        let frame_t0 = Instant::now();
        let prepare_us = (frame_t0 - loop_start).as_micros();
        update_slint_animations(animation_clock);
        let mut layer_target = LayerTarget::new(target, f, disp, ui);
        let frame_t1 = Instant::now();
        let this_rect = layer_target.render_slint_base(&window);
        let frame_t2 = Instant::now();
        let custom_draw_start = Instant::now();
        let full_frame_present = should_present_full_frame(launching, route_action);
        let arcade_list_update_start = Instant::now();
        let arcade_list_rect = if !launching && nav.screen == Screen::Arcade {
            let force_arcade_redraw =
                arcade_list_needs_forced_redraw(this_rect, full_frame_present);
            arcade_list_renderer.draw(
                active_arcade_games,
                nav.arcade.visual_index,
                force_arcade_redraw,
            )
        } else {
            None
        };
        let arcade_list_update_us = arcade_list_update_start.elapsed().as_micros();
        let preview_blit_start = Instant::now();
        let (raw_preview_rect, preview_transition_trace) = layer_target.blit_raw_preview_if_needed(
            &mut preview,
            &mut preview_transition,
            loop_start.duration_since(run_start),
            this_rect,
        );
        let preview_blit_us = preview_blit_start.elapsed().as_micros();
        if preview_transition_trace.active {
            window.request_redraw();
        }
        let effect_label_start = Instant::now();
        let effect_label_rect = effect_label_overlay.as_mut().map(|overlay| {
            layer_target.draw_effect_label(overlay, preview_transition_trace.effect.label())
        });
        let effect_label_us = effect_label_start.elapsed().as_micros();
        let custom_draw_trace = LauncherCustomDrawTrace {
            arcade_list_update_us,
            preview_blit_us,
            effect_label_us,
        };
        let custom_draw_done = Instant::now();
        if !first_render_logged {
            first_render_logged = true;
            boot_analytics::event(
                "first_render",
                format!("frame={frames} dirty_rect={}", format_dirty_rect(this_rect)),
            );
        }
        let pace = if frame_accounting.first_visible_copy_done() {
            let pace = pacer.wait();
            let frame_t3 = Instant::now();
            (Some(pace), frame_t3)
        } else {
            (None, Instant::now())
        };
        let frame_t3 = pace.1;
        let vsync_source = pace.0.as_ref().map(|pace| pace.source);
        let vsync_period_us = pace
            .0
            .as_ref()
            .map(|pace| pace.period_us)
            .unwrap_or_else(|| pacer.period_us());
        let vsync_miss_streak = pace.0.as_ref().map(|pace| pace.miss_streak).unwrap_or(0);
        if !first_vsync_logged
            && pace
                .0
                .as_ref()
                .is_some_and(|p| p.source == VsyncPaceSource::Vsync)
        {
            first_vsync_logged = true;
            boot_analytics::event("first_vsync", format!("frame={frames}"));
        }
        let (presentation, frame_t4) = {
            let presentation = LauncherCompositor::present(LauncherPresentRequest {
                layer_target: &mut layer_target,
                full_frame_present,
                slint_dirty: this_rect,
                raw_preview_rect,
                effect_label_rect,
                arcade_list_rect,
                arcade_list_renderer: &mut arcade_list_renderer,
                present_probe: present_probe.as_mut(),
                frames,
            });
            (presentation, Instant::now())
        };
        frame_accounting.finish_frame(
            LauncherPresentedFrame {
                frames,
                selected: nav.arcade.selected,
                visual_index: nav.arcade.visual_index,
                run_start,
                loop_start,
                frame_t0,
                frame_t1,
                frame_t2,
                frame_t3,
                frame_t4,
                custom_draw_start,
                custom_draw_done,
                custom_draw_trace,
                prepare_trace,
                prepare_us,
                dirty_rect: this_rect,
                copied_rows: presentation.copied_rows,
                cached_present_us: presentation.cached_present_us,
                overlay_present_us: presentation.overlay_present_us,
                present_probe_us: presentation.present_probe_us,
                vsync_source,
                vsync_period_us,
                vsync_miss_streak,
                arcade_update_label: presentation.arcade_update_label,
                preview_cache_state: preview.trace_cache_state(),
                preview_transition: preview_transition_trace,
                status_write_due,
                status_string_copy_us,
                status_string_copy_bytes,
            },
            start,
            disp,
            &nav,
            &pad,
            &catalog,
            catalog_ready,
            catalog_session.refresh_done(),
            launching,
            &loading_title,
            catalog_scan_visible,
            status_text
                .as_ref()
                .map(|text| text.catalog_scan_title.as_str())
                .unwrap_or(""),
            status_text
                .as_ref()
                .map(|text| text.catalog_scan_detail.as_str())
                .unwrap_or(""),
            catalog_scan_percent,
            catalog_background_scan_visible,
            status_text
                .as_ref()
                .map(|text| text.catalog_scan_message.as_str())
                .unwrap_or(""),
            confirm_visible,
            status_text
                .as_ref()
                .map(|text| text.confirm_title.as_str())
                .unwrap_or(""),
            confirm_selected,
            status_text
                .as_ref()
                .map(|text| text.confirm_left_label.as_str())
                .unwrap_or(""),
            status_text
                .as_ref()
                .map(|text| text.confirm_right_label.as_str())
                .unwrap_or(""),
            launcher_bench_scenario,
            start_screen,
            lock_screen,
            route_reassert_count,
            last_route_reassert_frame,
            last_route_reassert_ok,
            &last_route_reassert_error,
        );
        frames += 1;
    }
    let elapsed = run_start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Err(e) = cpu_profile::finish(cpu) {
        eprintln!("{e}");
    }
}

fn preview_scroll_exit_after_trace_deadline(run_start: Instant) -> Option<Instant> {
    if !matches!(
        std::env::var("MISTER_PREVIEW_SCROLL_EXIT_AFTER_TRACE").as_deref(),
        Ok("1") | Ok("on") | Ok("true") | Ok("yes")
    ) {
        return None;
    }
    let secs = std::env::var("MISTER_PREVIEW_SCROLL_TRACE_SECS")
        .ok()?
        .parse::<u64>()
        .ok()?;
    (secs > 0).then(|| run_start + Duration::from_secs(secs))
}

fn launcher_auto_launch_selected_enabled() -> bool {
    matches!(
        std::env::var("MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn apply_pending_launch_return_state(
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    pending: &mut Option<launcher::LaunchReturnState>,
) -> bool {
    let Some(state) = pending.take() else {
        return false;
    };
    launcher::apply_launch_return_state(nav, catalog, state)
}

#[allow(clippy::too_many_arguments)]
fn apply_catalog_session_effects(
    effects: CatalogSessionEffects,
    app: &slint_ui::launcher::Launcher,
    pad: &PadPool,
    nav: &mut LauncherNav,
    setup: &SetupNav,
    loading_title: &str,
    catalog: &mut ArcadeCatalog,
    catalog_ready: &mut bool,
    catalog_version: &mut usize,
    pending_launch_return_state: &mut Option<launcher::LaunchReturnState>,
    preview: &mut PreviewState,
    bridge_models: &mut LauncherBridgeModels,
    media_session: &mut ScreenshotMediaUpdateSession,
    media_handle: &mut Option<MediaWorkerHandle>,
    media_worker_unavailable: &mut bool,
    catalog_rx: &mut Option<mpsc::Receiver<CatalogWorkerMessage>>,
    full_bridge_dirty: &mut bool,
    start: Instant,
) {
    for effect in effects.into_effects() {
        match effect {
            CatalogSessionEffect::StartupEvent(event) => {
                print_startup_event(start, &event.name, event.detail);
            }
            CatalogSessionEffect::UseCatalog {
                catalog: ready_catalog,
                load_us: _,
            } => {
                *catalog = ready_catalog;
                *catalog_version = (*catalog_version).wrapping_add(1);
                *catalog_ready = true;
                apply_forced_arcade_selected(nav, catalog);
                apply_pending_launch_return_state(nav, catalog, pending_launch_return_state);
            }
            CatalogSessionEffect::SyncCatalogBridge => {
                let bridge_sync_t = Instant::now();
                sync_bridge_launcher(
                    app,
                    pad,
                    nav,
                    setup,
                    loading_title,
                    "",
                    Some(catalog),
                    preview,
                    bridge_models,
                    *catalog_version,
                    false,
                );
                print_startup_event(
                    start,
                    "catalog_bridge_sync_update",
                    format!(
                        "games={} elapsed_us={}",
                        catalog.len(),
                        bridge_sync_t.elapsed().as_micros()
                    ),
                );
            }
            CatalogSessionEffect::Ui(intent) => {
                apply_launcher_worker_ui_intent(app, intent, full_bridge_dirty);
            }
            CatalogSessionEffect::FinishMediaWorker => {
                apply_screenshot_media_update_effects(
                    media_session.finish_worker(),
                    app,
                    catalog,
                    media_handle,
                    media_worker_unavailable,
                    full_bridge_dirty,
                    start,
                );
            }
            CatalogSessionEffect::FinishMediaWorkerIfNoCatalogSeedPending => {
                apply_screenshot_media_update_effects(
                    media_session.finish_worker_if_no_catalog_seed_pending(),
                    app,
                    catalog,
                    media_handle,
                    media_worker_unavailable,
                    full_bridge_dirty,
                    start,
                );
            }
            CatalogSessionEffect::RequestMediaCatalogSeed => {
                media_session.request_catalog_seed();
            }
            CatalogSessionEffect::MediaSystemDiscovered {
                system_id,
                media_gate,
            } => {
                apply_screenshot_media_update_effects(
                    media_session.handle_catalog_system_discovered(system_id, media_gate),
                    app,
                    catalog,
                    media_handle,
                    media_worker_unavailable,
                    full_bridge_dirty,
                    start,
                );
            }
            CatalogSessionEffect::RequestLibraryRebuildOnNextBoot => {
                match launcher::request_library_rebuild_on_next_boot() {
                    Ok(()) => {
                        print_startup_event(start, "library_rebuild_deferred", "marker=written");
                    }
                    Err(e) => {
                        eprintln!("failed to defer library rebuild: {e}");
                        print_startup_event(start, "library_rebuild_defer_failed", e);
                    }
                }
            }
            CatalogSessionEffect::Confirm(action) => {
                nav.confirm_action = Some(action);
                nav.confirm_selected = 0;
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::StartCatalogWorker(worker) => {
                print_startup_event(start, "catalog_worker_start", &worker.root);
                *catalog_rx = Some(start_library_catalog_worker(
                    worker.root,
                    worker.request,
                    worker.initial_cache,
                ));
            }
        }
    }
}

fn apply_screenshot_media_update_effects(
    effects: ScreenshotMediaUpdateEffects,
    app: &slint_ui::launcher::Launcher,
    catalog: &ArcadeCatalog,
    media_handle: &mut Option<MediaWorkerHandle>,
    media_worker_unavailable: &mut bool,
    full_bridge_dirty: &mut bool,
    start: Instant,
) {
    for effect in effects.into_effects() {
        match effect {
            ScreenshotMediaUpdateEffect::StartupEvent(event) => {
                print_startup_event(start, &event.name, event.detail);
            }
            ScreenshotMediaUpdateEffect::Ui(intent) => {
                apply_launcher_worker_ui_intent(app, intent, full_bridge_dirty);
            }
            ScreenshotMediaUpdateEffect::EnsureWorker { mode } => {
                ensure_media_worker_started(media_handle, media_worker_unavailable, start, mode);
            }
            ScreenshotMediaUpdateEffect::EnsureSystem { system_id } => {
                if let Some(handle) = media_handle.as_ref() {
                    handle.ensure_system(&system_id);
                }
            }
            ScreenshotMediaUpdateEffect::EnsureCatalogSystems => {
                ensure_media_for_catalog_systems(
                    catalog,
                    media_handle,
                    media_worker_unavailable,
                    start,
                );
            }
            ScreenshotMediaUpdateEffect::FinishWorker => {
                if let Some(handle) = media_handle.as_ref() {
                    handle.finish();
                }
            }
            ScreenshotMediaUpdateEffect::DropWorker => {
                *media_handle = None;
            }
            ScreenshotMediaUpdateEffect::MarkWorkerUnavailable => {
                *media_worker_unavailable = true;
            }
            ScreenshotMediaUpdateEffect::SetInteractionActive { active, reason } => {
                if let Some(handle) = media_handle.as_ref() {
                    handle.set_interaction_active(active, reason);
                }
            }
        }
    }
}

fn ensure_media_worker_started(
    media_handle: &mut Option<MediaWorkerHandle>,
    media_worker_unavailable: &mut bool,
    start: Instant,
    mode: &str,
) {
    if media_handle.is_some() || *media_worker_unavailable {
        return;
    }
    *media_handle = start_screenshot_media_worker();
    if media_handle.is_some() {
        print_startup_event(
            start,
            "screenshot_media_worker_start",
            format!("mode={mode}"),
        );
    } else {
        *media_worker_unavailable = true;
        print_startup_event(
            start,
            "screenshot_media_worker_skip",
            format!("mode={mode}"),
        );
    }
}

fn ensure_media_for_catalog_systems(
    catalog: &ArcadeCatalog,
    media_handle: &mut Option<MediaWorkerHandle>,
    media_worker_unavailable: &mut bool,
    start: Instant,
) {
    let systems = catalog_media_system_ids(catalog);
    if systems.is_empty() {
        return;
    }
    ensure_media_worker_started(
        media_handle,
        media_worker_unavailable,
        start,
        "catalog-systems",
    );
    let Some(handle) = media_handle.as_ref() else {
        return;
    };
    for system_id in systems {
        print_startup_event(
            start,
            "screenshot_media_catalog_system_present",
            format!("system={system_id} source=catalog-seed"),
        );
        print_startup_event(
            start,
            "screenshot_media_catalog_ensure",
            format!("system={system_id}"),
        );
        handle.ensure_system(&system_id);
    }
}

fn catalog_media_system_ids(catalog: &ArcadeCatalog) -> Vec<String> {
    let mut seen = BTreeSet::new();
    catalog
        .systems
        .iter()
        .filter_map(|system| {
            let id = system.id.as_str();
            (mister_magik_fb::media_update::is_supported_pack_id(id)
                && catalog.system_game_count(id) > 0
                && seen.insert(system.id.clone()))
            .then(|| system.id.clone())
        })
        .collect()
}

#[derive(Debug)]
struct CatalogScanRedraw {
    last_request: Instant,
    period: Duration,
}

impl CatalogScanRedraw {
    fn new() -> Self {
        Self {
            last_request: Instant::now() - catalog_scan_redraw_period(),
            period: catalog_scan_redraw_period(),
        }
    }

    fn should_request(
        &mut self,
        visible: bool,
        background_visible: bool,
        percent: i32,
        now: Instant,
    ) -> bool {
        if !visible && !background_visible {
            return false;
        }
        if visible && !background_visible && percent >= 0 {
            return false;
        }
        if now.duration_since(self.last_request) < self.period {
            return false;
        }
        self.last_request = now;
        true
    }
}

fn catalog_scan_redraw_period() -> Duration {
    let fps = std::env::var("MISTER_CATALOG_SCAN_FPS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(15)
        .clamp(1, 60);
    Duration::from_millis((1000 / fps).max(1))
}

fn catalog_background_validation_delay() -> Duration {
    std::env::var("MISTER_CATALOG_BACKGROUND_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_CATALOG_BACKGROUND_VALIDATION_DELAY)
}

fn library_changed_test_dialog_choice_from_env(
    start: Instant,
) -> Option<launcher::LibraryChangedTestDialogChoice> {
    let value = std::env::var("MISTER_MAGIK_TEST_LIBRARY_CHANGED_DIALOG_CHOICE").ok()?;
    match launcher::parse_library_changed_test_dialog_choice(&value) {
        Ok(choice) => choice,
        Err(e) => {
            eprintln!("{e}");
            print_startup_event(start, "library_changed_test_dialog_choice_invalid", e);
            None
        }
    }
}

fn initial_catalog_scan_visible(
    catalog_ready: bool,
    _arcade_catalog_required_at_start: bool,
    catalog_worker_enabled: bool,
    foreground_update: bool,
) -> bool {
    catalog_worker_enabled && (foreground_update || !catalog_ready)
}

fn ready_catalog_worker_request(refresh_policy: CatalogRefreshPolicy) -> CatalogWorkerRequest {
    if refresh_policy == CatalogRefreshPolicy::Off {
        CatalogWorkerRequest::LoadOnly
    } else if refresh_policy.force_requested() {
        CatalogWorkerRequest::ForceBuild
    } else {
        CatalogWorkerRequest::CheckStamp
    }
}

fn launcher_bench_initial_preview_ready(
    scenario: LauncherBenchScenario,
    preview_cache_state: &str,
) -> bool {
    !scenario.starts_on_arcade() || matches!(preview_cache_state, "exact" | "empty")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(mister_experiments)]
    use crate::ui_effect_bench::{EffectFill, EffectTarget};
    #[cfg(mister_experiments)]
    use mister_magik_fb::effects::EffectSize;
    use std::sync::Arc;

    fn unique_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mister-magik-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[cfg(mister_experiments)]
    #[test]
    pub(super) fn effect_half_target_allows_640x448_at_native_scale() {
        let ui = UiDisplay::for_framebuffer(1920, 1080);
        let target = EffectTarget::new(EffectFill::Half, EffectSize { w: 640, h: 448 }, &ui)
            .expect("640x448 should fit in half-fill benchmark mode");

        assert_eq!(target.physical_w, 640);
        assert_eq!(target.physical_h, 448);
        assert_eq!(target.render_w, 640);
        assert_eq!(target.render_h, 448);
        assert_eq!(target.scale, 1);
    }

    #[test]
    pub(super) fn dirty_rect_ignores_fully_negative_bounds() {
        assert_eq!(dirty_rect_from_bounds(-40, 10, 20, 20, 960, 540), None);
        assert_eq!(dirty_rect_from_bounds(10, -40, 20, 20, 960, 540), None);
    }

    #[test]
    pub(super) fn dirty_rect_clips_partially_negative_bounds() {
        assert_eq!(
            dirty_rect_from_bounds(-10, -5, 30, 20, 960, 540),
            Some(DirtyRect {
                x0: 0,
                y0: 0,
                x1: 20,
                y1: 15
            })
        );
    }

    #[test]
    pub(super) fn dirty_rect_ignores_zero_area_bounds() {
        assert_eq!(dirty_rect_from_bounds(10, 10, 0, 20, 960, 540), None);
        assert_eq!(dirty_rect_from_bounds(10, 10, 20, 0, 960, 540), None);
    }

    #[test]
    pub(super) fn dirty_rect_keeps_in_bounds_rect() {
        assert_eq!(
            dirty_rect_from_bounds(10, 20, 30, 40, 960, 540),
            Some(DirtyRect {
                x0: 10,
                y0: 20,
                x1: 40,
                y1: 60
            })
        );
    }

    fn catalog_for_media_systems(system_ids: &[&str]) -> ArcadeCatalog {
        let mut games = Vec::new();
        let mut systems = Vec::new();
        for system_id in system_ids {
            games.push(ArcadeGameEntry {
                title: Arc::from(format!("{system_id} game")),
                mra_path: Arc::from(format!("/media/fat/_Arcade/{system_id}.mra")),
                preview_archive_path: Arc::from(format!(
                    "/media/fat/mister-magik/assets/{system_id}-screenshots.mmlz4b"
                )),
                preview_asset_key: Arc::from(format!("{system_id}.raw565")),
                has_preview: true,
                system_id: Arc::from(*system_id),
                is_new: false,
            });
            systems.push(arcade_catalog::GameSystemEntry {
                id: (*system_id).to_string(),
                title: (*system_id).to_string(),
                count: 1,
            });
        }
        ArcadeCatalog::new(PathBuf::from("/media/fat/_Arcade"), games, systems)
    }

    #[test]
    pub(super) fn catalog_media_system_ids_are_selective_and_supported() {
        let catalog = catalog_for_media_systems(&["arcade", "pcengine", "neogeo", "arcade"]);

        assert_eq!(
            catalog_media_system_ids(&catalog),
            vec!["arcade".to_string(), "neogeo".to_string()]
        );
    }

    #[test]
    pub(super) fn catalog_summary_seed_requires_usable_sqlite_database() {
        let root = unique_temp_dir("catalog-summary-seed");
        let db = root.join("library.sqlite3");
        let summary_path = catalog_summary::summary_path_for_sqlite(&db);
        let summary = catalog_summary::CatalogSummaryProjection {
            schema: catalog_summary::CATALOG_SUMMARY_SCHEMA_VERSION,
            catalog_schema_version: mister_magik_catalog::catalog_config::SCHEMA_VERSION,
            catalog_build_version: mister_magik_catalog::catalog_config::CATALOG_BUILD_VERSION,
            catalog_generation: "test-generation".to_string(),
            catalog_stamp_fingerprint: "test-generation".to_string(),
            catalog_stamp_lines: Vec::new(),
            total_game_count: 7,
            systems: vec![catalog_summary::CatalogSummarySystem {
                id: "arcade".to_string(),
                title: "Arcade".to_string(),
                count: 7,
                supported_media: vec!["screenshots".to_string()],
            }],
        };
        std::fs::write(
            &summary_path,
            serde_json::to_vec(&summary).expect("summary json"),
        )
        .expect("write summary");

        assert!(read_catalog_summary_seed(&db, &summary_path, Instant::now()).is_none());

        std::fs::write(&db, b"").expect("write zero-byte sqlite placeholder");
        assert!(read_catalog_summary_seed(&db, &summary_path, Instant::now()).is_none());

        std::fs::write(&db, b"not-a-sqlite-db").expect("write corrupt sqlite placeholder");
        assert!(read_catalog_summary_seed(&db, &summary_path, Instant::now()).is_none());

        std::fs::write(&db, SQLITE_HEADER).expect("write sqlite header");
        assert_eq!(
            read_catalog_summary_seed(&db, &summary_path, Instant::now()),
            Some(summary)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    pub(super) fn home_boot_with_ready_catalog_hides_catalog_popup() {
        assert!(!initial_catalog_scan_visible(true, false, true, false));
        assert!(initial_catalog_scan_visible(true, false, true, true));
    }

    #[test]
    pub(super) fn library_changed_test_driver_presses_continue_dialog_button() {
        let start = Instant::now();
        let mut nav = LauncherNav::new();
        let mut driver = LibraryChangedDialogTestDriver {
            choice: Some(launcher::LibraryChangedTestDialogChoice::Continue),
            dialog_seen_at: None,
            phase: LibraryChangedDialogTestPhase::Waiting,
        };

        assert!(driver.input_for(&nav, start, start).is_none());
        nav.confirm_action = Some(launcher::ConfirmAction::LibraryChanged);

        assert!(driver.input_for(&nav, start, start).is_none());
        let input = driver
            .input_for(&nav, start + LIBRARY_CHANGED_TEST_ACTION_SETTLE, start)
            .expect("continue driver should press A");
        assert!(input.btn_a);
        let event = nav
            .handle_input(
                &input,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE,
                &empty_arcade_catalog("/tmp"),
            )
            .expect("continue button should choose stale library");
        assert_eq!(event.action, LauncherAction::ContinueWithStaleLibrary);
        assert_eq!(nav.confirm_action, None);
    }

    #[test]
    pub(super) fn library_changed_test_driver_selects_rebuild_dialog_button() {
        let start = Instant::now();
        let mut nav = LauncherNav::new();
        nav.confirm_action = Some(launcher::ConfirmAction::LibraryChanged);
        let mut driver = LibraryChangedDialogTestDriver {
            choice: Some(launcher::LibraryChangedTestDialogChoice::Rebuild),
            dialog_seen_at: None,
            phase: LibraryChangedDialogTestPhase::Waiting,
        };
        let catalog = empty_arcade_catalog("/tmp");

        assert!(driver.input_for(&nav, start, start).is_none());
        let right = driver
            .input_for(&nav, start + LIBRARY_CHANGED_TEST_ACTION_SETTLE, start)
            .expect("rebuild driver should press right first");
        assert!(right.dpad_right);
        assert!(nav
            .handle_input(&right, start + LIBRARY_CHANGED_TEST_ACTION_SETTLE, &catalog)
            .is_none());
        assert_eq!(nav.confirm_selected, 1);

        let release = driver
            .input_for(
                &nav,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE + Duration::from_millis(16),
                start,
            )
            .expect("rebuild driver should release right before A");
        assert!(!release.dpad_right);
        assert!(!release.btn_a);
        assert!(nav
            .handle_input(
                &release,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE + Duration::from_millis(16),
                &catalog,
            )
            .is_none());

        let press_a = driver
            .input_for(
                &nav,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE + Duration::from_millis(32),
                start,
            )
            .expect("rebuild driver should press A");
        assert!(press_a.btn_a);
        let event = nav
            .handle_input(
                &press_a,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE + Duration::from_millis(32),
                &catalog,
            )
            .expect("A should confirm rebuild");
        assert_eq!(event.action, LauncherAction::RebuildLibrary);
        assert_eq!(nav.confirm_action, None);
    }

    #[test]
    pub(super) fn arcade_bench_waits_for_initial_visible_preview() {
        let scenario = LauncherBenchScenario::HeldScroll;

        assert!(!launcher_bench_initial_preview_ready(
            scenario,
            "placeholder"
        ));
        assert!(!launcher_bench_initial_preview_ready(scenario, "cached"));
        assert!(!launcher_bench_initial_preview_ready(scenario, "stale"));
        assert!(launcher_bench_initial_preview_ready(scenario, "exact"));
        assert!(launcher_bench_initial_preview_ready(scenario, "empty"));
    }

    #[test]
    pub(super) fn non_arcade_bench_does_not_wait_for_preview() {
        assert!(launcher_bench_initial_preview_ready(
            LauncherBenchScenario::HomeNav,
            "placeholder"
        ));
    }

    #[test]
    pub(super) fn missing_catalog_shows_catalog_popup_on_home_or_arcade_boot() {
        assert!(initial_catalog_scan_visible(false, false, true, false));
        assert!(initial_catalog_scan_visible(false, true, true, false));
        assert!(!initial_catalog_scan_visible(true, true, true, false));
        assert!(!initial_catalog_scan_visible(false, true, false, false));
    }

    #[test]
    pub(super) fn ready_catalog_rebuild_progress_uses_background_badge() {
        for title in ["Indexing library", "Loading library"] {
            let full_visible = catalog_scan_progress_visible(true, Screen::Home, title, false);
            assert!(!full_visible, "{title} should not cover a ready catalog");
            assert!(catalog_background_scan_progress_visible(
                true,
                full_visible,
                title
            ));
        }
    }

    #[test]
    pub(super) fn catalog_scan_redraw_throttles_indeterminate_animation() {
        let now = Instant::now();
        let mut redraw = CatalogScanRedraw {
            last_request: now,
            period: Duration::from_millis(66),
        };
        assert!(!redraw.should_request(true, false, -1, now + Duration::from_millis(20)));
        assert!(redraw.should_request(true, false, -1, now + Duration::from_millis(70)));
    }

    #[test]
    pub(super) fn catalog_scan_redraw_skips_determinate_periodic_frames() {
        let now = Instant::now();
        let mut redraw = CatalogScanRedraw {
            last_request: now - Duration::from_secs(1),
            period: Duration::from_millis(66),
        };
        assert!(!redraw.should_request(true, false, 90, now));
        assert!(!redraw.should_request(false, false, -1, now));
    }

    #[test]
    pub(super) fn catalog_scan_redraw_animates_background_badge() {
        let now = Instant::now();
        let mut redraw = CatalogScanRedraw {
            last_request: now - Duration::from_secs(1),
            period: Duration::from_millis(66),
        };
        assert!(redraw.should_request(false, true, -1, now));
    }

    #[test]
    pub(super) fn cached_home_validation_progress_stays_hidden() {
        assert!(!catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Validating library",
            false
        ));
        assert!(!catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Preview images changed",
            false
        ));
        assert!(catalog_background_scan_progress_visible(
            true,
            false,
            "Validating library"
        ));
        assert!(catalog_background_scan_progress_visible(
            true,
            false,
            "Checking library"
        ));
    }

    #[test]
    pub(super) fn missing_catalog_and_rebuild_progress_are_visible() {
        assert!(catalog_scan_progress_visible(
            false,
            Screen::Home,
            "Indexing library",
            false
        ));
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Indexing library",
            true
        ));
        assert!(!catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Library changed",
            false
        ));
        assert!(!catalog_background_scan_progress_visible(
            true,
            true,
            "Indexing library"
        ));
    }

    #[test]
    pub(super) fn catalog_scan_failures_are_visible_even_with_cache() {
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Library scan failed",
            false
        ));
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Arcade,
            "Library load failed",
            false
        ));
        assert!(!catalog_background_scan_progress_visible(
            true,
            true,
            "Library scan failed"
        ));
    }

    #[test]
    pub(super) fn ready_catalog_uses_background_worker_for_refresh_or_home_validation() {
        assert_eq!(
            ready_catalog_worker_request(CatalogRefreshPolicy::Default),
            CatalogWorkerRequest::CheckStamp
        );
        assert_eq!(
            ready_catalog_worker_request(CatalogRefreshPolicy::Force),
            CatalogWorkerRequest::ForceBuild
        );
        assert_eq!(
            ready_catalog_worker_request(CatalogRefreshPolicy::Off),
            CatalogWorkerRequest::LoadOnly
        );
    }
}
