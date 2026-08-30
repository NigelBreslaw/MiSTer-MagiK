// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Immutable process-boundary configuration capture.

#[cfg(feature = "ui")]
use crate::cpu_profile::CpuProfileConfig;
#[cfg(feature = "ui")]
use crate::frame_profile::FrameProfilerConfig;
#[cfg(feature = "ui")]
use crate::launcher_runtime::media::MediaWorkerConfig;
#[cfg(feature = "ui")]
use crate::preview_state::PreviewStateConfig;
#[cfg(feature = "ui")]
use crate::screenshot_transitions::PreviewTransitionConfig;
#[cfg(feature = "ui")]
use crate::ui_display::UiDisplayInputs;
#[cfg(feature = "ui")]
use crate::ui_runner::latch_v5_qualification::QualificationConfig;
#[cfg(feature = "ui")]
use crate::ui_runner::launcher_bench::LauncherBenchmarkConfig;
#[cfg(feature = "ui")]
use crate::ui_runner::launcher_gui_profile::GuiProfileConfig;
#[cfg(feature = "ui")]
use crate::visual_platform::{AnimationClockConfig, PresentTiming};
use mister_magik_catalog::catalog_config::ArchiveCacheConfig;
use mister_magik_catalog::device_layout::{CatalogPathOverrides, CatalogPaths, DevicePaths};
use mister_magik_catalog::fs_fault::FaultConfig;
#[cfg(feature = "ui")]
use mister_magik_mister_runtime::framebuffer::ownership::FramebufferRouteConfig;
#[cfg(feature = "ui")]
use mister_magik_mister_runtime::framebuffer::target::DirtyRegionConfig;
#[cfg(feature = "ui")]
use mister_magik_mister_runtime::framebuffer::vsync::VsyncPacerConfig;
#[cfg(feature = "ui")]
use mister_magik_perf_events::PmuProfileConfig;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

const STARTUP_TOKEN: &str = "MISTER_MAGIK_STARTUP_TOKEN";
const READY_FIFO: &str = "MISTER_MAGIK_READY_FIFO";
const READY_WIRE_VERSION: &str = "MISTER_MAGIK_READY_WIRE_VERSION";
const MAIN_PID: &str = "MISTER_MAGIK_MAIN_PID";
const MAIN_GENERATION: &str = "MISTER_MAGIK_MAIN_GENERATION";
const OWNER_EPOCH: &str = "MISTER_MAGIK_OWNER_EPOCH";
const LAUNCHER_RESPONSE_TRACE: &str = "MISTER_LAUNCHER_RESPONSE_TRACE";
const LAUNCHER_RESPONSE_EXECUTION_TRACE: &str = "MISTER_LAUNCHER_RESPONSE_EXECUTION_TRACE";
const LAUNCHER_RESPONSE_PMU: &str = "MISTER_LAUNCHER_RESPONSE_PMU";
const LAUNCHER_RESPONSE_COMPLETE: &str = "MISTER_LAUNCHER_RESPONSE_COMPLETE";
const LAUNCHER_RESPONSE_FRAME_COMPLETE: &str = "MISTER_LAUNCHER_RESPONSE_FRAME_COMPLETE";
const LAUNCHER_RESPONSE_PMU_COMPLETE: &str = "MISTER_LAUNCHER_RESPONSE_PMU_COMPLETE";
const LAUNCHER_RESPONSE_RUN_ID: &str = "MISTER_LAUNCHER_RESPONSE_RUN_ID";
const LAUNCHER_RESPONSE_EXPECTED_CONFIRMED: &str = "MISTER_LAUNCHER_RESPONSE_EXPECTED_CONFIRMED";
const LAUNCHER_RESPONSE_EXPECTED_FEEDBACK_HIDDEN: &str =
    "MISTER_LAUNCHER_RESPONSE_EXPECTED_FEEDBACK_HIDDEN";
const SYSTEM_ENTRY_RUN_ID: &str = "MISTER_SYSTEM_ENTRY_RUN_ID";
const ARCADE_ENTRY_RUN_ID: &str = "MISTER_ARCADE_ENTRY_RUN_ID";
const SYSTEM_ENTRY_TRACE: &str = "MISTER_SYSTEM_ENTRY_TRACE";
const ARCADE_ENTRY_TRACE: &str = "MISTER_ARCADE_ENTRY_TRACE";
const SYSTEM_ENTRY_PROFILE_OUT: &str = "MISTER_SYSTEM_ENTRY_PROFILE_OUT";
const INPUT_INTEGRITY_STALL_MS: &str = "MISTER_INPUT_INTEGRITY_STALL_MS";
const INPUT_INTEGRITY_TRACE: &str = "MISTER_INPUT_INTEGRITY_TRACE";
#[cfg(any(feature = "bench-tools", test))]
const LAUNCHER_INPUT_SCRIPT: &str = "MISTER_LAUNCHER_INPUT_SCRIPT";
#[cfg(any(feature = "bench-tools", test))]
const LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES: &str = "MISTER_LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES";
const SCREENSAVER_SEED: &str = "MISTER_SCREENSAVER_SEED";
const SCREENSAVER_START_ACTIVE: &str = "MISTER_SCREENSAVER_START_ACTIVE";
const SCREENSAVER_START_IDLE_WHEN_READY: &str = "MISTER_SCREENSAVER_START_IDLE_WHEN_READY";
const SCREENSAVER_START_PREVIEW_AFTER_ANALYTICS: &str =
    "MISTER_SCREENSAVER_START_PREVIEW_AFTER_ANALYTICS";
const SCREENSAVER_START_PREVIEW_WHEN_READY: &str = "MISTER_SCREENSAVER_START_PREVIEW_WHEN_READY";
const PRESENT_BACKEND: &str = "MISTER_PRESENT_BACKEND";
const TEST_CATALOG_RECOVERY_DIALOG: &str = "MISTER_MAGIK_TEST_CATALOG_RECOVERY_DIALOG";
const TEST_LIBRARY_CHANGED_DIALOG_CHOICE: &str = "MISTER_MAGIK_TEST_LIBRARY_CHANGED_DIALOG_CHOICE";
const TEST_AUTO_LAUNCH_GATE: &str = "MISTER_MAGIK_TEST_AUTO_LAUNCH_GATE";
const TEST_CATALOG_PUBLICATION_GATE: &str = "MISTER_MAGIK_TEST_CATALOG_PUBLICATION_GATE";
const TEST_FIRST_FRAME_RELEASE_GATE: &str = "MISTER_MAGIK_TEST_FIRST_FRAME_RELEASE_GATE";
const TEST_CATALOG_PUBLICATION_SESSION: &str = "MISTER_MAGIK_TEST_CATALOG_PUBLICATION_SESSION";
const TEST_STARTUP_MODE: &str = "MISTER_UI_TEST_STARTUP_MODE";
const MODAL_TEST_PATH_INPUTS: &[&str] = &[
    "MISTER_SHARDED_CATALOG_DIR",
    "MISTER_LIBRARY_SQLITE",
    "MISTER_LIBRARY_REFRESH_LOCK",
    "MISTER_CATALOG_READY_SNAPSHOT",
    "MISTER_CATALOG_DIAGNOSTICS_DIR",
];

#[derive(Clone, Default)]
pub struct EnvironmentSnapshot {
    values: BTreeMap<OsString, OsString>,
}

impl EnvironmentSnapshot {
    pub fn capture_process() -> Self {
        Self {
            values: std::env::vars_os().collect(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .get(OsStr::new(name))
            .and_then(|value| value.to_str())
    }

    pub fn get_path(&self, name: &str) -> Option<&Path> {
        self.values.get(OsStr::new(name)).map(Path::new)
    }

    #[cfg(test)]
    fn from_values(values: impl IntoIterator<Item = (&'static str, &'static str)>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandMode {
    Ui,
    LatchReadinessReport,
    Other(String),
}

impl CommandMode {
    fn from_name(command: &str) -> Self {
        match command {
            "ui" => Self::Ui,
            "latch-readiness-report" => Self::LatchReadinessReport,
            other => Self::Other(other.to_owned()),
        }
    }

    fn captures_launcher(&self) -> bool {
        matches!(self, Self::Ui)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum InstrumentationModifier {
    Json,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InstrumentationModifiers {
    enabled: BTreeSet<InstrumentationModifier>,
}

impl InstrumentationModifiers {
    fn from_args(command: &CommandMode, args: &[String]) -> Self {
        let mut enabled = BTreeSet::new();
        if matches!(command, CommandMode::LatchReadinessReport)
            && args.iter().any(|arg| arg == "--json")
        {
            enabled.insert(InstrumentationModifier::Json);
        }
        Self { enabled }
    }

    pub fn contains(&self, modifier: InstrumentationModifier) -> bool {
        self.enabled.contains(&modifier)
    }
}

#[derive(Clone)]
pub struct LauncherProcessConfig {
    readiness: LauncherReadinessConfig,
    device_paths: DevicePaths,
    catalog_paths: CatalogPaths,
    archive_cache: ArchiveCacheConfig,
    #[cfg(feature = "ui")]
    preview: PreviewStateConfig,
    #[cfg(feature = "ui")]
    preview_transition: PreviewTransitionConfig,
    #[cfg(feature = "ui")]
    media_worker: Result<MediaWorkerConfig, String>,
    screensaver: ScreensaverProcessConfig,
    input: InputProcessConfig,
    #[cfg(feature = "ui")]
    display_pacing: DisplayPacingConfig,
    #[cfg(feature = "ui")]
    profiles: ProfileProcessConfig,
    #[cfg(feature = "ui")]
    benchmark: LauncherBenchmarkConfig,
    #[cfg(feature = "ui")]
    qualification: QualificationConfig,
    tests: LauncherTestConfig,
    presentation_backend: PresentBackendConfig,
}

impl LauncherProcessConfig {
    pub fn readiness(&self) -> &LauncherReadinessConfig {
        &self.readiness
    }

    pub fn device_paths(&self) -> &DevicePaths {
        &self.device_paths
    }

    pub fn catalog_paths(&self) -> &CatalogPaths {
        &self.catalog_paths
    }

    pub fn archive_cache(&self) -> &ArchiveCacheConfig {
        &self.archive_cache
    }

    #[cfg(feature = "ui")]
    pub fn preview(&self) -> &PreviewStateConfig {
        &self.preview
    }

    #[cfg(feature = "ui")]
    pub fn preview_transition(&self) -> &PreviewTransitionConfig {
        &self.preview_transition
    }

    #[cfg(feature = "ui")]
    pub fn media_worker(&self) -> &Result<MediaWorkerConfig, String> {
        &self.media_worker
    }

    pub fn screensaver(&self) -> &ScreensaverProcessConfig {
        &self.screensaver
    }

    pub fn input(&self) -> &InputProcessConfig {
        &self.input
    }

    #[cfg(feature = "ui")]
    pub fn display_pacing(&self) -> &DisplayPacingConfig {
        &self.display_pacing
    }

    #[cfg(feature = "ui")]
    pub fn profiles(&self) -> &ProfileProcessConfig {
        &self.profiles
    }

    #[cfg(feature = "ui")]
    pub fn benchmark(&self) -> &LauncherBenchmarkConfig {
        &self.benchmark
    }

    #[cfg(feature = "ui")]
    pub fn qualification(&self) -> QualificationConfig {
        self.qualification
    }

    pub fn tests(&self) -> &LauncherTestConfig {
        &self.tests
    }

    pub fn presentation_backend(&self) -> &PresentBackendConfig {
        &self.presentation_backend
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LauncherTestConfig {
    catalog_recovery_dialog: Option<String>,
    library_changed_dialog_choice: Option<String>,
    auto_launch_gate: Option<PathBuf>,
    catalog_publication_gate: Option<PathBuf>,
    first_frame_release_gate: Option<PathBuf>,
    catalog_publication_session: Option<PathBuf>,
    startup_mode: Option<LauncherStartupTestMode>,
    modal_path_inputs: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LauncherStartupTestMode {
    WarmReady,
    ColdDelayed,
    ColdIntroFailure,
}

impl LauncherStartupTestMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "warm-ready" => Some(Self::WarmReady),
            "cold-delayed" => Some(Self::ColdDelayed),
            "cold-intro-failure" => Some(Self::ColdIntroFailure),
            _ => None,
        }
    }
}

impl LauncherTestConfig {
    fn capture(environment: &EnvironmentSnapshot) -> Self {
        Self {
            catalog_recovery_dialog: environment
                .get(TEST_CATALOG_RECOVERY_DIALOG)
                .map(str::to_owned),
            library_changed_dialog_choice: environment
                .get(TEST_LIBRARY_CHANGED_DIALOG_CHOICE)
                .map(str::to_owned),
            auto_launch_gate: environment
                .get_path(TEST_AUTO_LAUNCH_GATE)
                .map(Path::to_path_buf),
            catalog_publication_gate: volatile_test_path(
                environment.get_path(TEST_CATALOG_PUBLICATION_GATE),
            ),
            first_frame_release_gate: volatile_test_path(
                environment.get_path(TEST_FIRST_FRAME_RELEASE_GATE),
            ),
            catalog_publication_session: volatile_test_path(
                environment.get_path(TEST_CATALOG_PUBLICATION_SESSION),
            ),
            startup_mode: environment
                .get(TEST_STARTUP_MODE)
                .and_then(LauncherStartupTestMode::parse),
            modal_path_inputs: MODAL_TEST_PATH_INPUTS
                .iter()
                .filter_map(|name| environment.get_path(name).map(Path::to_path_buf))
                .collect(),
        }
    }

    pub fn catalog_recovery_dialog(&self) -> Option<&str> {
        self.catalog_recovery_dialog.as_deref()
    }

    pub fn library_changed_dialog_choice(&self) -> Option<&str> {
        self.library_changed_dialog_choice.as_deref()
    }

    pub fn auto_launch_gate(&self) -> Option<&Path> {
        self.auto_launch_gate.as_deref()
    }

    pub fn catalog_publication_gate(&self) -> Option<&Path> {
        self.catalog_publication_gate.as_deref()
    }

    pub fn first_frame_release_gate(&self) -> Option<&Path> {
        self.first_frame_release_gate.as_deref()
    }

    pub fn catalog_publication_session(&self) -> Option<&Path> {
        self.catalog_publication_session.as_deref()
    }

    pub fn startup_mode(&self) -> Option<LauncherStartupTestMode> {
        self.startup_mode
    }

    pub fn modal_path_inputs(&self) -> &[PathBuf] {
        &self.modal_path_inputs
    }
}

fn volatile_test_path(path: Option<&Path>) -> Option<PathBuf> {
    path.filter(|path| path.starts_with("/tmp") && path != &Path::new("/tmp"))
        .map(Path::to_path_buf)
}

#[derive(Clone, Default)]
pub struct FaultProcessConfig {
    armed: Option<FaultConfig>,
}

impl FaultProcessConfig {
    fn capture(environment: &EnvironmentSnapshot) -> Self {
        let captured = FaultConfig::capture_with(|name| environment.get(name));
        let armed = captured.filter(|config| {
            config
                .session_token()
                .is_some_and(|path| Path::new(path).starts_with("/tmp") && path != "/tmp")
        });
        Self { armed }
    }

    pub fn armed(&self) -> Option<&FaultConfig> {
        self.armed.as_ref()
    }
}

#[cfg(feature = "ui")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileProcessConfig {
    frame: FrameProfilerConfig,
    cpu: CpuProfileConfig,
    gui: GuiProfileConfig,
    pmu: PmuProfileConfig,
}

#[cfg(feature = "ui")]
impl ProfileProcessConfig {
    fn capture(environment: &EnvironmentSnapshot) -> Self {
        Self {
            frame: FrameProfilerConfig::capture_with(|name| environment.get(name)),
            cpu: CpuProfileConfig::capture_with(|name| environment.get(name)),
            gui: GuiProfileConfig::capture_with(|name| environment.get(name)),
            pmu: PmuProfileConfig::capture_with(|name| environment.get(name)),
        }
    }

    pub fn frame(&self) -> &FrameProfilerConfig {
        &self.frame
    }

    pub fn cpu(&self) -> &CpuProfileConfig {
        &self.cpu
    }

    pub(crate) fn gui(&self) -> &GuiProfileConfig {
        &self.gui
    }

    pub fn pmu(&self) -> PmuProfileConfig {
        self.pmu
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PresentBackendConfig {
    FpgaVblankLatchHidden,
    Fb0Dirty,
    Retired(String),
    Invalid(String),
}

impl PresentBackendConfig {
    fn capture(environment: &EnvironmentSnapshot) -> Self {
        match environment.get(PRESENT_BACKEND) {
            None | Some("") | Some("fpga-vblank-latch-hidden") => Self::FpgaVblankLatchHidden,
            Some("fb0-dirty") => Self::Fb0Dirty,
            Some(value) if present_backend_is_retired(value) => Self::Retired(value.to_owned()),
            Some(value) => Self::Invalid(value.to_owned()),
        }
    }
}

fn present_backend_is_retired(value: &str) -> bool {
    value == ["main", "flip-v1"].join("-")
        || value == ["main", "vsync-hidden"].join("-")
        || value == ["plugin", "main", "vsync-hidden"].join("-")
}

#[cfg(feature = "ui")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayPacingConfig {
    display_inputs: UiDisplayInputs,
    animation_clock: AnimationClockConfig,
    present_timing: PresentTiming,
    vsync: VsyncPacerConfig,
    dirty_region: DirtyRegionConfig,
    route: FramebufferRouteConfig,
}

#[cfg(feature = "ui")]
impl DisplayPacingConfig {
    fn capture(environment: &EnvironmentSnapshot) -> Self {
        Self {
            display_inputs: UiDisplayInputs::capture_with(|name| environment.get(name)),
            animation_clock: AnimationClockConfig::capture_environment_with(|name| {
                environment.get(name)
            }),
            present_timing: PresentTiming::capture_with(|name| environment.get(name)),
            vsync: VsyncPacerConfig::capture_with(|name| environment.get(name)),
            dirty_region: DirtyRegionConfig::capture_with(|name| environment.get(name)),
            route: FramebufferRouteConfig::capture_with(|name| environment.get(name)),
        }
    }

    pub fn display_inputs(&self) -> &UiDisplayInputs {
        &self.display_inputs
    }

    pub fn animation_clock(&self) -> &AnimationClockConfig {
        &self.animation_clock
    }

    pub fn present_timing(&self) -> PresentTiming {
        self.present_timing
    }

    pub fn vsync(&self) -> &VsyncPacerConfig {
        &self.vsync
    }

    pub fn dirty_rect_broad_pct(&self) -> usize {
        self.dirty_region.broad_pct()
    }

    pub fn route_reassert_frames(&self) -> u64 {
        self.route.reassert_interval_frames()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreensaverStartMode {
    Inactive,
    IdleWhenReady,
    PreviewWhenReady,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreensaverProcessConfig {
    start_mode: ScreensaverStartMode,
    preview_waits_for_analytics: bool,
    seed: Option<u64>,
}

impl ScreensaverProcessConfig {
    fn capture(environment: &EnvironmentSnapshot) -> Self {
        let idle_when_ready = environment_flag(environment, SCREENSAVER_START_IDLE_WHEN_READY);
        let preview_when_ready =
            environment_flag(environment, SCREENSAVER_START_PREVIEW_WHEN_READY);
        let legacy_start_active = environment_flag(environment, SCREENSAVER_START_ACTIVE);
        let start_mode = if preview_when_ready {
            ScreensaverStartMode::PreviewWhenReady
        } else if idle_when_ready {
            ScreensaverStartMode::IdleWhenReady
        } else if legacy_start_active {
            ScreensaverStartMode::PreviewWhenReady
        } else {
            ScreensaverStartMode::Inactive
        };
        Self {
            start_mode,
            preview_waits_for_analytics: environment_flag(
                environment,
                SCREENSAVER_START_PREVIEW_AFTER_ANALYTICS,
            ),
            seed: environment
                .get(SCREENSAVER_SEED)
                .and_then(parse_screensaver_seed),
        }
    }

    pub fn start_mode(&self) -> ScreensaverStartMode {
        self.start_mode
    }

    pub fn preview_waits_for_analytics(&self) -> bool {
        self.preview_waits_for_analytics
    }

    pub fn seed(&self) -> Option<u64> {
        self.seed
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScriptedInputConfig {
    script: Option<String>,
    wait_frames: usize,
}

impl ScriptedInputConfig {
    pub fn script(&self) -> Option<&str> {
        self.script.as_deref()
    }

    pub fn wait_frames(&self) -> usize {
        self.wait_frames
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InputProcessConfig {
    integrity_trace: bool,
    integrity_stall_ms: Option<u64>,
    scripted: ScriptedInputConfig,
}

impl InputProcessConfig {
    fn capture(environment: &EnvironmentSnapshot) -> Self {
        #[cfg(feature = "bench-tools")]
        let scripted = ScriptedInputConfig {
            script: environment.get(LAUNCHER_INPUT_SCRIPT).map(str::to_owned),
            wait_frames: environment
                .get(LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(60)
                .min(600),
        };
        #[cfg(not(feature = "bench-tools"))]
        let scripted = ScriptedInputConfig::default();
        Self {
            integrity_trace: environment_flag(environment, INPUT_INTEGRITY_TRACE),
            integrity_stall_ms: environment
                .get(INPUT_INTEGRITY_STALL_MS)
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| (1..=1_000).contains(value)),
            scripted,
        }
    }

    pub fn integrity_trace(&self) -> bool {
        self.integrity_trace
    }

    pub fn integrity_stall_ms(&self) -> Option<u64> {
        self.integrity_stall_ms
    }

    pub fn scripted(&self) -> &ScriptedInputConfig {
        &self.scripted
    }
}

#[derive(Clone, Default)]
pub struct LauncherReadinessConfig {
    startup_token: String,
    ready_fifo: PathBuf,
    ready_wire_version: u8,
    main_pid: u32,
    main_generation: u64,
    owner_epoch: u64,
    response_trace: LauncherResponseTraceConfig,
    entry_trace: LauncherEntryTraceConfig,
}

impl LauncherReadinessConfig {
    fn from_snapshot(environment: &EnvironmentSnapshot) -> Self {
        Self {
            startup_token: environment
                .get(STARTUP_TOKEN)
                .unwrap_or_default()
                .to_owned(),
            ready_fifo: environment
                .get_path(READY_FIFO)
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            ready_wire_version: match environment.get(READY_WIRE_VERSION) {
                Some("3") => 3,
                _ => 2,
            },
            main_pid: parse_u32(environment.get(MAIN_PID)),
            main_generation: parse_u64(environment.get(MAIN_GENERATION)),
            owner_epoch: parse_u64(environment.get(OWNER_EPOCH)),
            response_trace: LauncherResponseTraceConfig::capture(environment),
            entry_trace: LauncherEntryTraceConfig::capture(environment),
        }
    }

    pub fn response_trace(&self) -> &LauncherResponseTraceConfig {
        &self.response_trace
    }

    pub fn entry_trace(&self) -> &LauncherEntryTraceConfig {
        &self.entry_trace
    }

    pub fn into_parts(self) -> (String, PathBuf, u8, u32, u64, u64) {
        (
            self.startup_token,
            self.ready_fifo,
            self.ready_wire_version,
            self.main_pid,
            self.main_generation,
            self.owner_epoch,
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LauncherResponseTraceConfig {
    enabled: bool,
    execution_enabled: bool,
    pmu_enabled: bool,
    completion_path: Option<String>,
    frame_completion_path: Option<String>,
    pmu_completion_path: Option<String>,
    run_id: String,
    expected_confirmed: usize,
    expected_feedback_hidden: usize,
}

impl LauncherResponseTraceConfig {
    fn capture(environment: &EnvironmentSnapshot) -> Self {
        let enabled = environment_flag(environment, LAUNCHER_RESPONSE_TRACE);
        Self {
            enabled,
            execution_enabled: enabled
                && environment_flag(environment, LAUNCHER_RESPONSE_EXECUTION_TRACE),
            pmu_enabled: enabled && environment_flag(environment, LAUNCHER_RESPONSE_PMU),
            completion_path: environment
                .get(LAUNCHER_RESPONSE_COMPLETE)
                .map(str::to_owned),
            frame_completion_path: environment
                .get(LAUNCHER_RESPONSE_FRAME_COMPLETE)
                .map(str::to_owned),
            pmu_completion_path: environment
                .get(LAUNCHER_RESPONSE_PMU_COMPLETE)
                .map(str::to_owned),
            run_id: environment
                .get(LAUNCHER_RESPONSE_RUN_ID)
                .unwrap_or_default()
                .to_owned(),
            expected_confirmed: response_expected_count(
                environment.get(LAUNCHER_RESPONSE_EXPECTED_CONFIRMED),
                enabled,
            ),
            expected_feedback_hidden: response_expected_count(
                environment.get(LAUNCHER_RESPONSE_EXPECTED_FEEDBACK_HIDDEN),
                enabled,
            ),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn execution_enabled(&self) -> bool {
        self.execution_enabled
    }

    pub fn pmu_enabled(&self) -> bool {
        self.pmu_enabled
    }

    pub fn completion_path(&self) -> Option<&str> {
        self.completion_path.as_deref()
    }

    pub fn frame_completion_path(&self) -> Option<&str> {
        self.frame_completion_path.as_deref()
    }

    pub fn pmu_completion_path(&self) -> Option<&str> {
        self.pmu_completion_path.as_deref()
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn expected_confirmed(&self) -> usize {
        self.expected_confirmed
    }

    pub fn expected_feedback_hidden(&self) -> usize {
        self.expected_feedback_hidden
    }
}

fn response_expected_count(value: Option<&str>, enabled: bool) -> usize {
    if !enabled {
        return 0;
    }
    value
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|count| *count <= 256)
        .unwrap_or(0)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LauncherEntryTraceConfig {
    run_id: String,
    trace_path: Option<String>,
    profile_path: Option<String>,
}

impl LauncherEntryTraceConfig {
    fn capture(environment: &EnvironmentSnapshot) -> Self {
        Self {
            run_id: environment
                .get(SYSTEM_ENTRY_RUN_ID)
                .or_else(|| environment.get(ARCADE_ENTRY_RUN_ID))
                .unwrap_or_default()
                .to_owned(),
            trace_path: environment
                .get(SYSTEM_ENTRY_TRACE)
                .or_else(|| environment.get(ARCADE_ENTRY_TRACE))
                .filter(|path| !path.is_empty())
                .map(str::to_owned),
            profile_path: environment
                .get(SYSTEM_ENTRY_PROFILE_OUT)
                .filter(|path| !path.is_empty())
                .map(str::to_owned),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn trace_path(&self) -> Option<&str> {
        self.trace_path.as_deref()
    }

    pub fn profile_path(&self) -> Option<&str> {
        self.profile_path.as_deref()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiagnosticConfig {
    pub latch_readiness_json: bool,
}

#[derive(Clone)]
pub struct ProcessConfig {
    command: CommandMode,
    device_paths: DevicePaths,
    catalog_paths: CatalogPaths,
    archive_cache: ArchiveCacheConfig,
    launcher: Option<LauncherProcessConfig>,
    instrumentation: InstrumentationModifiers,
    fault: FaultProcessConfig,
}

impl ProcessConfig {
    pub fn capture(args: &[String], command: &str) -> Self {
        let device_paths = DevicePaths::current();
        let environment = EnvironmentSnapshot::capture_process();
        Self::from_snapshot_with_device_paths(args, command, &environment, device_paths)
    }

    #[cfg(test)]
    fn from_snapshot(args: &[String], command: &str, environment: &EnvironmentSnapshot) -> Self {
        Self::from_snapshot_with_device_paths(
            args,
            command,
            environment,
            DevicePaths::for_layout(mister_magik_platform_manifest_contract::Layout::Public),
        )
    }

    fn from_snapshot_with_device_paths(
        args: &[String],
        command: &str,
        environment: &EnvironmentSnapshot,
        device_paths: DevicePaths,
    ) -> Self {
        let command = CommandMode::from_name(command);
        let instrumentation = InstrumentationModifiers::from_args(&command, args);
        let catalog_paths = CatalogPaths::derive(
            &device_paths,
            CatalogPathOverrides::capture_with(|name| environment.get_path(name)),
        );
        let archive_cache = ArchiveCacheConfig::capture_with(
            &catalog_paths,
            |name| environment.get_path(name),
            |name| environment.get(name),
        );
        let launcher = command.captures_launcher().then(|| LauncherProcessConfig {
            readiness: LauncherReadinessConfig::from_snapshot(environment),
            device_paths: device_paths.clone(),
            catalog_paths: catalog_paths.clone(),
            archive_cache: archive_cache.clone(),
            #[cfg(feature = "ui")]
            preview: PreviewStateConfig::capture_with(|name| environment.get(name)),
            #[cfg(feature = "ui")]
            preview_transition: PreviewTransitionConfig::capture_with(|name| environment.get(name)),
            #[cfg(feature = "ui")]
            media_worker: MediaWorkerConfig::capture_with(&catalog_paths, |name| {
                environment.get(name)
            }),
            screensaver: ScreensaverProcessConfig::capture(environment),
            input: InputProcessConfig::capture(environment),
            #[cfg(feature = "ui")]
            display_pacing: DisplayPacingConfig::capture(environment),
            #[cfg(feature = "ui")]
            profiles: ProfileProcessConfig::capture(environment),
            #[cfg(feature = "ui")]
            benchmark: LauncherBenchmarkConfig::capture_with(|name| environment.get(name)),
            #[cfg(feature = "ui")]
            qualification: QualificationConfig::capture_with(|name| environment.get(name)),
            tests: LauncherTestConfig::capture(environment),
            presentation_backend: PresentBackendConfig::capture(environment),
        });
        let fault = FaultProcessConfig::capture(environment);
        Self {
            command,
            device_paths,
            catalog_paths,
            archive_cache,
            launcher,
            instrumentation,
            fault,
        }
    }

    pub fn command(&self) -> &CommandMode {
        &self.command
    }

    pub fn device_paths(&self) -> &DevicePaths {
        &self.device_paths
    }

    pub fn catalog_paths(&self) -> &CatalogPaths {
        &self.catalog_paths
    }

    pub fn archive_cache(&self) -> &ArchiveCacheConfig {
        &self.archive_cache
    }

    pub fn launcher(&self) -> Option<&LauncherProcessConfig> {
        self.launcher.as_ref()
    }

    pub fn instrumentation(&self) -> &InstrumentationModifiers {
        &self.instrumentation
    }

    pub fn diagnostics(&self) -> DiagnosticConfig {
        DiagnosticConfig {
            latch_readiness_json: self.instrumentation.contains(InstrumentationModifier::Json),
        }
    }

    pub fn fault(&self) -> Option<&FaultConfig> {
        self.fault.armed()
    }
}

fn environment_flag(environment: &EnvironmentSnapshot, name: &str) -> bool {
    matches!(environment.get(name), Some("1" | "on" | "true" | "yes"))
}

fn parse_screensaver_seed(value: &str) -> Option<u64> {
    let value = value.trim();
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map(|digits| u64::from_str_radix(digits, 16).ok())
        .unwrap_or_else(|| value.parse::<u64>().ok())
}

fn parse_u32(value: Option<&str>) -> u32 {
    value.and_then(|value| value.parse().ok()).unwrap_or(0)
}

fn parse_u64(value: Option<&str>) -> u64 {
    value.and_then(|value| value.parse().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_capture_preserves_compatible_values_and_invalid_defaults() {
        let environment = EnvironmentSnapshot::from_values([
            (STARTUP_TOKEN, "0123456789abcdef0123456789abcdef"),
            (READY_FIFO, "/tmp/ready"),
            (READY_WIRE_VERSION, "invalid"),
            (MAIN_PID, "7"),
            (MAIN_GENERATION, "11"),
            (OWNER_EPOCH, "invalid"),
        ]);
        let config = ProcessConfig::from_snapshot(
            &["mister-magik-fb".into(), "ui".into()],
            "ui",
            &environment,
        );
        assert_eq!(config.command(), &CommandMode::Ui);
        assert_eq!(
            config
                .launcher()
                .expect("ui captures launcher configuration")
                .readiness()
                .clone()
                .into_parts(),
            (
                "0123456789abcdef0123456789abcdef".into(),
                PathBuf::from("/tmp/ready"),
                2,
                7,
                11,
                0,
            )
        );
    }

    #[test]
    fn readiness_capture_accepts_wire_version_three() {
        let environment = EnvironmentSnapshot::from_values([
            (STARTUP_TOKEN, "0123456789abcdef0123456789abcdef"),
            (READY_FIFO, "/tmp/ready"),
            (READY_WIRE_VERSION, "3"),
            (MAIN_PID, "7"),
            (MAIN_GENERATION, "11"),
            (OWNER_EPOCH, "13"),
        ]);
        let config = ProcessConfig::from_snapshot(
            &["mister-magik-fb".into(), "ui".into()],
            "ui",
            &environment,
        );
        assert_eq!(
            config
                .launcher()
                .unwrap()
                .readiness()
                .clone()
                .into_parts()
                .2,
            3
        );
    }

    #[test]
    fn diagnostic_modifier_is_scoped_to_the_readiness_command() {
        let args = vec![
            "mister-magik-fb".into(),
            "ui".into(),
            "--json".into(),
            "--json".into(),
        ];
        let ui = ProcessConfig::from_snapshot(&args, "ui", &EnvironmentSnapshot::default());
        assert!(!ui.diagnostics().latch_readiness_json);
        assert!(ui.launcher().is_some());
        let report = ProcessConfig::from_snapshot(
            &args,
            "latch-readiness-report",
            &EnvironmentSnapshot::default(),
        );
        assert!(report.diagnostics().latch_readiness_json);
        assert!(report.launcher().is_none());
        assert!(
            report
                .instrumentation()
                .contains(InstrumentationModifier::Json)
        );
    }

    #[test]
    fn unrelated_commands_do_not_capture_launcher_or_readiness_settings() {
        let environment = EnvironmentSnapshot::from_values([
            (STARTUP_TOKEN, "secret"),
            (READY_FIFO, "/tmp/ready"),
            (MAIN_PID, "7"),
        ]);
        let config = ProcessConfig::from_snapshot(
            &["mister-magik-fb".into(), "catalog-inspect".into()],
            "catalog-inspect",
            &environment,
        );

        assert_eq!(
            config.command(),
            &CommandMode::Other("catalog-inspect".into())
        );
        assert!(config.launcher().is_none());
        assert_eq!(
            config.instrumentation(),
            &InstrumentationModifiers::default()
        );
    }

    #[test]
    fn launcher_paths_preserve_executable_layout_and_root_remapping() {
        use mister_magik_platform_manifest_contract::Layout;

        for (layout, root) in [
            (Layout::Public, Path::new("/media/fat")),
            (Layout::Development, Path::new("/media/fat")),
            (Layout::Development, Path::new("/tmp/card")),
        ] {
            let installed = layout.paths();
            let canonical_root = Path::new(installed.root)
                .parent()
                .expect("installed app root has a device parent");
            let remap = |canonical: &str| {
                root.join(
                    Path::new(canonical)
                        .strip_prefix(canonical_root)
                        .expect("installed path remains under the device root"),
                )
            };
            let config = ProcessConfig::from_snapshot_with_device_paths(
                &["mister-magik-fb".into(), "ui".into()],
                "ui",
                &EnvironmentSnapshot::default(),
                DevicePaths::remapped(layout, root),
            );
            let launcher = config
                .launcher()
                .expect("ui captures launcher configuration");
            assert_eq!(launcher.device_paths(), config.device_paths());
            assert_eq!(launcher.catalog_paths(), config.catalog_paths());
            assert_eq!(launcher.archive_cache(), config.archive_cache());
            assert_eq!(launcher.device_paths().main_path(), remap(installed.main));
            assert_eq!(
                launcher.device_paths().app_path("settings.json"),
                remap(installed.root).join("settings.json")
            );
        }
    }

    #[test]
    fn catalog_paths_capture_overrides_once_at_the_command_boundary() {
        let environment = EnvironmentSnapshot::from_values([
            ("MISTER_LIBRARY_SQLITE", "/tmp/catalog/library.sqlite3"),
            ("MISTER_PREVIEW_CACHE_DIR", "/tmp/catalog/previews"),
            ("MISTER_SHARDED_CATALOG_DIR", "/tmp/catalog/v3"),
        ]);
        let config = ProcessConfig::from_snapshot_with_device_paths(
            &["mister-magik-fb".into(), "ui".into()],
            "ui",
            &environment,
            DevicePaths::remapped(
                mister_magik_platform_manifest_contract::Layout::Development,
                "/tmp/card",
            ),
        );

        assert_eq!(
            config.catalog_paths().library_sqlite(),
            Path::new("/tmp/catalog/library.sqlite3")
        );
        assert_eq!(
            config.catalog_paths().preview_cache_dir(),
            Path::new("/tmp/catalog/previews")
        );
        assert_eq!(
            config.catalog_paths().sharded_catalog_dir(),
            Path::new("/tmp/catalog/v3")
        );
        assert_eq!(
            config.catalog_paths().media_asset_dir(),
            config.device_paths().app_path("assets")
        );
        assert_eq!(
            config.archive_cache().preview_cache_dir(),
            config.catalog_paths().preview_cache_dir()
        );
        assert_eq!(
            config.archive_cache().sqlite_build_dir(),
            config.catalog_paths().library_sqlite_build_dir()
        );
    }

    #[test]
    #[cfg(feature = "ui")]
    fn launcher_captures_preview_media_and_transition_settings_once() {
        let environment = EnvironmentSnapshot::from_values([
            ("MISTER_PREVIEW_LOADING", "off"),
            ("MISTER_PREVIEW_TURBO_LOOKAHEAD", "999"),
            ("MISTER_PREVIEW_VISUAL_PCT", "5"),
            ("MISTER_PREVIEW_ARCHIVES", "/tmp/a.mmlz4b:/tmp/b.mmlz4b"),
            ("MISTER_PREVIEW_TRANSITION", "fade,slide-left"),
            ("MISTER_PREVIEW_TRANSITION_MS", "9999"),
            ("MISTER_MEDIA_UPDATE", "off"),
            ("MISTER_MEDIA_SIZE", "320x320"),
        ]);
        let config = ProcessConfig::from_snapshot(
            &["mister-magik-fb".into(), "ui".into()],
            "ui",
            &environment,
        );
        let launcher = config.launcher().expect("ui captures launcher settings");

        assert_eq!(launcher.preview().visual_pct(), 10);
        assert_eq!(
            launcher.preview().worker().archive_paths(),
            &["/tmp/a.mmlz4b".to_string(), "/tmp/b.mmlz4b".to_string()]
        );
        assert_eq!(
            launcher.preview_transition(),
            &PreviewTransitionConfig::capture_with(|name| environment.get(name))
        );
        assert!(launcher.media_worker().is_ok());
    }

    #[test]
    fn launcher_captures_screensaver_and_input_integrity_settings_once() {
        let environment = EnvironmentSnapshot::from_values([
            (SCREENSAVER_START_ACTIVE, "true"),
            (SCREENSAVER_START_IDLE_WHEN_READY, "true"),
            (SCREENSAVER_START_PREVIEW_WHEN_READY, "true"),
            (SCREENSAVER_START_PREVIEW_AFTER_ANALYTICS, "yes"),
            (SCREENSAVER_SEED, "0x2a"),
            (INPUT_INTEGRITY_TRACE, "on"),
            (INPUT_INTEGRITY_STALL_MS, "1001"),
        ]);
        let config = ProcessConfig::from_snapshot(
            &["mister-magik-fb".into(), "ui".into()],
            "ui",
            &environment,
        );
        let launcher = config.launcher().expect("ui captures launcher settings");

        assert_eq!(
            launcher.screensaver().start_mode(),
            ScreensaverStartMode::PreviewWhenReady
        );
        assert!(launcher.screensaver().preview_waits_for_analytics());
        assert_eq!(launcher.screensaver().seed(), Some(42));
        assert!(launcher.input().integrity_trace());
        assert_eq!(launcher.input().integrity_stall_ms(), None);
    }

    #[test]
    #[cfg(not(feature = "bench-tools"))]
    fn production_configuration_cannot_arm_scripted_input() {
        let environment = EnvironmentSnapshot::from_values([
            (LAUNCHER_INPUT_SCRIPT, "left,a"),
            (LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES, "1"),
        ]);
        let config = ProcessConfig::from_snapshot(
            &["mister-magik-fb".into(), "ui".into()],
            "ui",
            &environment,
        );
        let scripted = config
            .launcher()
            .expect("ui captures launcher settings")
            .input()
            .scripted();

        assert_eq!(scripted, &ScriptedInputConfig::default());
    }

    #[test]
    fn fault_configuration_requires_a_volatile_session_token() {
        let ordinary = EnvironmentSnapshot::from_values([
            ("MISTER_FS_FAULT_POINT", "settings.after_rename"),
            ("MISTER_FS_FAULT_ACTION", "direct-reset-no-sync"),
        ]);
        let persistent = EnvironmentSnapshot::from_values([
            ("MISTER_FS_FAULT_POINT", "settings.after_rename"),
            ("MISTER_FS_FAULT_SESSION", "/media/fat/launcher.env"),
        ]);
        let volatile = EnvironmentSnapshot::from_values([
            ("MISTER_FS_FAULT_POINT", "settings.after_rename"),
            (
                "MISTER_FS_FAULT_SESSION",
                "/tmp/mister-magik/fs-fault-session",
            ),
        ]);

        assert!(FaultProcessConfig::capture(&ordinary).armed().is_none());
        assert!(FaultProcessConfig::capture(&persistent).armed().is_none());
        assert!(FaultProcessConfig::capture(&volatile).armed().is_some());
    }

    #[test]
    fn launcher_test_paths_reject_persistent_publication_controls() {
        let environment = EnvironmentSnapshot::from_values([
            (TEST_CATALOG_PUBLICATION_GATE, "/media/fat/gate"),
            (TEST_FIRST_FRAME_RELEASE_GATE, "/tmp/release"),
            (TEST_CATALOG_PUBLICATION_SESSION, "/tmp/session"),
        ]);
        let config = LauncherTestConfig::capture(&environment);

        assert!(config.catalog_publication_gate().is_none());
        assert_eq!(
            config.first_frame_release_gate(),
            Some(Path::new("/tmp/release"))
        );
        assert_eq!(
            config.catalog_publication_session(),
            Some(Path::new("/tmp/session"))
        );
    }

    #[test]
    fn startup_ui_test_modes_are_captured_as_typed_values() {
        for (value, expected) in [
            ("warm-ready", Some(LauncherStartupTestMode::WarmReady)),
            ("cold-delayed", Some(LauncherStartupTestMode::ColdDelayed)),
            (
                "cold-intro-failure",
                Some(LauncherStartupTestMode::ColdIntroFailure),
            ),
            ("unexpected", None),
        ] {
            let environment = EnvironmentSnapshot::from_values([(TEST_STARTUP_MODE, value)]);
            assert_eq!(
                LauncherTestConfig::capture(&environment).startup_mode(),
                expected
            );
        }
    }

    #[test]
    fn presentation_backend_is_validated_at_capture_boundary() {
        for (value, expected) in [
            ("", PresentBackendConfig::FpgaVblankLatchHidden),
            ("fb0-dirty", PresentBackendConfig::Fb0Dirty),
            (
                "fpga-vblank-latch-hidden",
                PresentBackendConfig::FpgaVblankLatchHidden,
            ),
            (
                "main-flip-v1",
                PresentBackendConfig::Retired("main-flip-v1".to_owned()),
            ),
            (
                "unknown",
                PresentBackendConfig::Invalid("unknown".to_owned()),
            ),
        ] {
            let environment = EnvironmentSnapshot::from_values([(PRESENT_BACKEND, value)]);
            assert_eq!(PresentBackendConfig::capture(&environment), expected);
        }
    }

    #[test]
    fn launcher_captures_trace_and_readiness_evidence_once() {
        let environment = EnvironmentSnapshot::from_values([
            (LAUNCHER_RESPONSE_TRACE, "1"),
            (LAUNCHER_RESPONSE_EXECUTION_TRACE, "yes"),
            (LAUNCHER_RESPONSE_COMPLETE, "/tmp/response.json"),
            (LAUNCHER_RESPONSE_EXPECTED_CONFIRMED, "17"),
            (LAUNCHER_RESPONSE_EXPECTED_FEEDBACK_HIDDEN, "999"),
            (SYSTEM_ENTRY_RUN_ID, "system-run"),
            (ARCADE_ENTRY_RUN_ID, "legacy-run"),
            (SYSTEM_ENTRY_TRACE, "/tmp/system-entry.tsv"),
            (SYSTEM_ENTRY_PROFILE_OUT, "/tmp/system-entry.json"),
        ]);
        let config = ProcessConfig::from_snapshot(
            &["mister-magik-fb".into(), "ui".into()],
            "ui",
            &environment,
        );
        let readiness = config
            .launcher()
            .expect("ui captures launcher settings")
            .readiness();

        assert!(readiness.response_trace().enabled());
        assert!(readiness.response_trace().execution_enabled());
        assert_eq!(readiness.response_trace().expected_confirmed(), 17);
        assert_eq!(readiness.response_trace().expected_feedback_hidden(), 0);
        assert_eq!(readiness.entry_trace().run_id(), "system-run");
        assert_eq!(
            readiness.entry_trace().trace_path(),
            Some("/tmp/system-entry.tsv")
        );
        assert_eq!(
            readiness.entry_trace().profile_path(),
            Some("/tmp/system-entry.json")
        );
    }
}
