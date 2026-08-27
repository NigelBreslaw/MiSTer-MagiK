// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Optional CPU sampling profiler (`--features profile`, env `MISTER_PPROF=1`).
//!
//! Uses `SIGPROF`/`ITIMER_PROF` sampling from the `pprof` crate — no `perf` CLI required.
//! Canonical device delivery includes this dormant feature. The offline full-debug artifact is
//! built with `scripts/agent build runtime-analysis`; benchmarks profile the installed runtime.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

const SCREENSAVER_TRIGGER: &str = "screensaver";
const NAVIGATION_TRANSITIONS_TRIGGER: &str = "navigation-transitions";
const SETTINGS_NAVIGATION_TRANSITIONS_TRIGGER: &str = "settings-navigation-transitions";
const ORIENTATION_TRANSITION_FADE_TRIGGER: &str = "orientation-transition-fade";
const ORIENTATION_TRANSITION_ZOOM_TRIGGER: &str = "orientation-transition-zoom";
const LAUNCH_RETURN_TRIGGER: &str = "launch-return";
const COLD_BOOT_TRIGGER: &str = "cold-boot";
const COLD_BOOT_CATALOG_TRIGGER: &str = "cold-boot-catalog";
const SYSTEM_ENTRY_TRIGGER: &str = "system-entry";
const LAUNCHER_RESPONSE_TRIGGER: &str = "launcher-response";
const ARCADE_VELOCITY_SCROLL_TRIGGER: &str = "arcade-velocity-scroll";
const CATALOG_BUILD_TRIGGER: &str = "catalog-build";
const CATALOG_BUILD_FULL_TRIGGER: &str = "catalog-build-full";
const PPROF: &str = "MISTER_PPROF";
const PPROF_TRIGGER: &str = "MISTER_PPROF_TRIGGER";
const PPROF_DURATION_SECS: &str = "MISTER_PPROF_DURATION_SECS";
const PPROF_WARMUP_SECS: &str = "MISTER_PPROF_WARMUP_SECS";
const PPROF_HZ: &str = "MISTER_PPROF_HZ";
const PPROF_OUT: &str = "MISTER_PPROF_OUT";
const PPROF_FOLDED_OUT: &str = "MISTER_PPROF_FOLDED_OUT";
const PPROF_COMPLETE: &str = "MISTER_PPROF_COMPLETE";
const DEFAULT_SCREENSAVER_PROFILE_SECS: u64 = 30;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BoundedProfileTrigger {
    Screensaver,
    NavigationTransitions,
    SettingsNavigationTransitions,
    OrientationTransitionFade,
    OrientationTransitionZoom,
    LauncherResponse,
    ArcadeVelocityScroll,
    CatalogBuild,
    LaunchReturn,
    ColdBoot,
}

impl BoundedProfileTrigger {
    #[cfg(feature = "profile")]
    const fn label(self) -> &'static str {
        match self {
            Self::Screensaver => SCREENSAVER_TRIGGER,
            Self::NavigationTransitions => NAVIGATION_TRANSITIONS_TRIGGER,
            Self::SettingsNavigationTransitions => SETTINGS_NAVIGATION_TRANSITIONS_TRIGGER,
            Self::OrientationTransitionFade => ORIENTATION_TRANSITION_FADE_TRIGGER,
            Self::OrientationTransitionZoom => ORIENTATION_TRANSITION_ZOOM_TRIGGER,
            Self::LauncherResponse => LAUNCHER_RESPONSE_TRIGGER,
            Self::ArcadeVelocityScroll => ARCADE_VELOCITY_SCROLL_TRIGGER,
            Self::CatalogBuild => CATALOG_BUILD_TRIGGER,
            Self::LaunchReturn => LAUNCH_RETURN_TRIGGER,
            Self::ColdBoot => COLD_BOOT_TRIGGER,
        }
    }

    #[cfg(feature = "profile")]
    const fn schema(self) -> &'static str {
        match self {
            Self::Screensaver => "mister-magik-screensaver-pprof-v1",
            Self::NavigationTransitions => "mister-magik-navigation-transitions-pprof-v1",
            Self::SettingsNavigationTransitions => {
                "mister-magik-settings-navigation-transitions-pprof-v4"
            }
            Self::OrientationTransitionFade => "mister-magik-orientation-transition-fade-pprof-v1",
            Self::OrientationTransitionZoom => "mister-magik-orientation-transition-zoom-pprof-v1",
            Self::LauncherResponse => "mister-magik-launcher-response-pprof-v1",
            Self::ArcadeVelocityScroll => "mister-magik-arcade-velocity-scroll-pprof-v1",
            Self::CatalogBuild => "mister-magik-catalog-build-pprof-v1",
            Self::LaunchReturn => "mister-magik-launch-return-pprof-v1",
            Self::ColdBoot => "mister-magik-cold-boot-pprof-v1",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum ScreensaverProfileState {
    Disabled,
    Waiting,
    Active,
    Complete,
    Failed,
}

impl ScreensaverProfileState {
    const fn label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Waiting => "waiting",
            Self::Active => "active",
            Self::Complete => "complete",
            Self::Failed => "failed",
        }
    }
}

static SCREENSAVER_PROFILE_STATE: AtomicU8 = AtomicU8::new(ScreensaverProfileState::Disabled as u8);

fn set_screensaver_profile_state(state: ScreensaverProfileState) {
    SCREENSAVER_PROFILE_STATE.store(state as u8, Ordering::Relaxed);
}

pub fn screensaver_profile_state() -> &'static str {
    match SCREENSAVER_PROFILE_STATE.load(Ordering::Relaxed) {
        value if value == ScreensaverProfileState::Waiting as u8 => {
            ScreensaverProfileState::Waiting.label()
        }
        value if value == ScreensaverProfileState::Active as u8 => {
            ScreensaverProfileState::Active.label()
        }
        value if value == ScreensaverProfileState::Complete as u8 => {
            ScreensaverProfileState::Complete.label()
        }
        value if value == ScreensaverProfileState::Failed as u8 => {
            ScreensaverProfileState::Failed.label()
        }
        _ => ScreensaverProfileState::Disabled.label(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuProfileConfig {
    enabled: bool,
    trigger: Option<BoundedProfileTrigger>,
    system_entry: bool,
    duration: Duration,
    warmup: Duration,
    hz: i32,
    out_path: String,
    folded_out_path: Option<String>,
    complete_path: Option<String>,
    catalog_build_full: bool,
    cold_boot_catalog: bool,
}

impl CpuProfileConfig {
    pub fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        let enabled_value = get(PPROF);
        let trigger_value = get(PPROF_TRIGGER);
        let trigger = bounded_profile_trigger_from_values(enabled_value, trigger_value);
        Self {
            enabled: enabled_value == Some("1"),
            trigger,
            system_entry: system_entry_profile_requested_from_values(enabled_value, trigger_value),
            duration: if trigger == Some(BoundedProfileTrigger::CatalogBuild) {
                catalog_build_profile_timeout_from_value(get(PPROF_DURATION_SECS))
            } else {
                screensaver_profile_duration_from_value(get(PPROF_DURATION_SECS))
            },
            warmup: screensaver_profile_warmup_from_value(get(PPROF_WARMUP_SECS)),
            hz: get(PPROF_HZ)
                .and_then(|value| value.parse().ok())
                .unwrap_or(99),
            out_path: get(PPROF_OUT).unwrap_or("/tmp/mister-pprof.svg").to_owned(),
            folded_out_path: get(PPROF_FOLDED_OUT).map(str::to_owned),
            complete_path: get(PPROF_COMPLETE).map(str::to_owned),
            catalog_build_full: enabled_value == Some("1")
                && trigger_value == Some(CATALOG_BUILD_FULL_TRIGGER),
            cold_boot_catalog: enabled_value == Some("1")
                && trigger_value == Some(COLD_BOOT_CATALOG_TRIGGER),
        }
    }

    pub fn navigation_transition_requested(&self) -> bool {
        matches!(
            self.trigger,
            Some(
                BoundedProfileTrigger::NavigationTransitions
                    | BoundedProfileTrigger::SettingsNavigationTransitions
            )
        )
    }

    pub fn launch_return_requested(&self) -> bool {
        self.trigger == Some(BoundedProfileTrigger::LaunchReturn)
    }

    pub fn cold_boot_requested(&self) -> bool {
        self.trigger == Some(BoundedProfileTrigger::ColdBoot)
    }

    pub fn cold_boot_catalog_requested(&self) -> bool {
        self.cold_boot_catalog
    }

    #[cfg(feature = "profile")]
    fn catalog_build_requested(&self) -> bool {
        self.trigger == Some(BoundedProfileTrigger::CatalogBuild)
    }

    #[cfg(feature = "profile")]
    fn ordinary_requested(&self) -> bool {
        self.enabled && self.trigger.is_none() && !self.system_entry
    }
}

fn system_entry_profile_requested_from_values(
    enabled: Option<&str>,
    trigger: Option<&str>,
) -> bool {
    enabled == Some("1") && trigger == Some(SYSTEM_ENTRY_TRIGGER)
}

fn bounded_profile_trigger_from_values(
    enabled: Option<&str>,
    trigger: Option<&str>,
) -> Option<BoundedProfileTrigger> {
    if enabled != Some("1") {
        return None;
    }
    match trigger {
        Some(SCREENSAVER_TRIGGER) => Some(BoundedProfileTrigger::Screensaver),
        Some(NAVIGATION_TRANSITIONS_TRIGGER) => Some(BoundedProfileTrigger::NavigationTransitions),
        Some(SETTINGS_NAVIGATION_TRANSITIONS_TRIGGER) => {
            Some(BoundedProfileTrigger::SettingsNavigationTransitions)
        }
        Some(ORIENTATION_TRANSITION_FADE_TRIGGER) => {
            Some(BoundedProfileTrigger::OrientationTransitionFade)
        }
        Some(ORIENTATION_TRANSITION_ZOOM_TRIGGER) => {
            Some(BoundedProfileTrigger::OrientationTransitionZoom)
        }
        Some(LAUNCHER_RESPONSE_TRIGGER) => Some(BoundedProfileTrigger::LauncherResponse),
        Some(ARCADE_VELOCITY_SCROLL_TRIGGER) => Some(BoundedProfileTrigger::ArcadeVelocityScroll),
        Some(CATALOG_BUILD_TRIGGER | CATALOG_BUILD_FULL_TRIGGER) => {
            Some(BoundedProfileTrigger::CatalogBuild)
        }
        Some(LAUNCH_RETURN_TRIGGER) => Some(BoundedProfileTrigger::LaunchReturn),
        Some(COLD_BOOT_TRIGGER | COLD_BOOT_CATALOG_TRIGGER) => {
            Some(BoundedProfileTrigger::ColdBoot)
        }
        _ => None,
    }
}

fn screensaver_profile_warmup_from_value(value: Option<&str>) -> Duration {
    Duration::from_secs(
        value
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
            .clamp(0, 300),
    )
}

fn screensaver_profile_duration_from_value(value: Option<&str>) -> Duration {
    Duration::from_secs(
        value
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_SCREENSAVER_PROFILE_SECS)
            .clamp(1, 300),
    )
}

fn catalog_build_profile_timeout_from_value(value: Option<&str>) -> Duration {
    Duration::from_secs(
        value
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(600)
            .clamp(1, 1_200),
    )
}

#[cfg(any(feature = "profile", test))]
fn screensaver_profile_frame_bounds(first_frame: u64, next_frame: u64) -> (u64, u64) {
    (first_frame, next_frame.saturating_sub(1).max(first_frame))
}

#[derive(Debug, Clone)]
pub struct CpuProfileSummary {
    pub sample_stacks: usize,
    pub sample_hits: isize,
    pub stackless_sample_hits: isize,
    pub duration_secs: f64,
    pub hz: i32,
    pub out_path: String,
    pub bytes: u64,
}

#[cfg(feature = "profile")]
mod imp {
    use super::{
        BoundedProfileTrigger, CpuProfileConfig, CpuProfileSummary, ScreensaverProfileState,
        screensaver_profile_frame_bounds, set_screensaver_profile_state,
    };
    use serde_json::json;
    use std::fs;
    use std::time::{Duration, Instant};

    pub struct CpuProfiler {
        guard: pprof::ProfilerGuard<'static>,
        hz: i32,
        out_path: String,
        folded_out_path: Option<String>,
    }

    pub fn start(config: &CpuProfileConfig) -> Option<CpuProfiler> {
        config
            .ordinary_requested()
            .then(|| start_enabled(config))
            .flatten()
    }

    pub fn start_system_entry(config: &CpuProfileConfig) -> Option<CpuProfiler> {
        config.system_entry.then(|| start_enabled(config)).flatten()
    }

    pub fn start_process_entry(config: &CpuProfileConfig) -> Option<CpuProfiler> {
        matches!(
            config.trigger,
            Some(BoundedProfileTrigger::LaunchReturn | BoundedProfileTrigger::ColdBoot)
        )
        .then(|| start_enabled(config))
        .flatten()
    }

    fn start_enabled(config: &CpuProfileConfig) -> Option<CpuProfiler> {
        // pprof restores the signal disposition it observed here after stopping its
        // ITIMER_PROF timer.  A final SIGPROF can already be pending at that point,
        // especially at the 999 Hz launch-return sampling rate.  Leaving SIGPROF at
        // its default disposition between sessions would therefore terminate the
        // launcher immediately after a successful profile.  Make the disposition
        // pprof restores harmless; pprof still installs its own handler while active.
        //
        // SAFETY: changing a process signal disposition through libc is valid here;
        // profiler sessions are serialized by pprof's global guard and no profiling
        // timer is active while start_enabled installs this between-session action.
        if unsafe { libc::signal(libc::SIGPROF, libc::SIG_IGN) } == libc::SIG_ERR {
            crate::ui_errln!(
                "cpu_profile: failed to make the between-session SIGPROF disposition safe: {}",
                std::io::Error::last_os_error()
            );
            return None;
        }
        let hz = config.hz;
        let out_path = config.out_path.clone();
        let folded_out_path = config.folded_out_path.clone();
        for path in std::iter::once(out_path.as_str()).chain(folded_out_path.as_deref()) {
            if let Some(parent) = std::path::Path::new(path).parent()
                && let Err(error) = fs::create_dir_all(parent)
            {
                crate::ui_errln!(
                    "cpu_profile: failed to create profile directory {}: {error}",
                    parent.display()
                );
                return None;
            }
        }
        crate::ui_logln!("cpu_profile: sampling at {hz} Hz → {out_path}");
        if let Some(path) = folded_out_path.as_deref() {
            crate::ui_logln!("cpu_profile: folded stacks → {path}");
        }
        match pprof::ProfilerGuard::new(hz) {
            Ok(guard) => Some(CpuProfiler {
                guard,
                hz,
                out_path,
                folded_out_path,
            }),
            Err(e) => {
                crate::ui_errln!("cpu_profile: ProfilerGuard::new failed: {e}");
                None
            }
        }
    }

    pub fn finish(profiler: Option<CpuProfiler>) -> Result<Option<CpuProfileSummary>, String> {
        let Some(p) = profiler else { return Ok(None) };
        let report = match p.guard.report().build() {
            Ok(r) => r,
            Err(e) => return Err(format!("cpu_profile: report build failed: {e}")),
        };
        let sample_stacks = report.data.len();
        let sample_hits: isize = report.data.values().sum();
        let stackless_sample_hits: isize = report
            .data
            .iter()
            .filter(|(frames, _)| frames.frames.is_empty())
            .map(|(_, hits)| hits)
            .sum();
        let duration_secs = report.timing.duration.as_secs_f64();
        crate::ui_logln!(
            "cpu_profile: {} unique stacks, {} sample hits, {} stackless, {:.1}s at {} Hz",
            sample_stacks,
            sample_hits,
            stackless_sample_hits,
            duration_secs,
            p.hz
        );
        if sample_hits == 0 {
            return Err(
                "cpu_profile: no CPU samples collected from SIGPROF/ITIMER_PROF timer".into(),
            );
        }
        if let Some(path) = p.folded_out_path.as_deref() {
            write_folded_report(&report, path)?;
        }
        match std::fs::File::create(&p.out_path) {
            Ok(mut file) => {
                if let Err(e) = report.flamegraph(&mut file) {
                    return Err(format!("cpu_profile: flamegraph write failed: {e}"));
                }
                let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
                crate::ui_logln!(
                    "cpu_profile: wrote flamegraph to {} ({bytes} bytes)",
                    p.out_path
                );
                Ok(Some(CpuProfileSummary {
                    sample_stacks,
                    sample_hits,
                    stackless_sample_hits,
                    duration_secs,
                    hz: p.hz,
                    out_path: p.out_path,
                    bytes,
                }))
            }
            Err(e) => Err(format!("cpu_profile: create {} failed: {e}", p.out_path)),
        }
    }

    pub fn finish_launch_return(
        profiler: Option<CpuProfiler>,
        complete_path: Option<&str>,
    ) -> Result<Option<CpuProfileSummary>, String> {
        let result = finish(profiler);
        let metadata = match &result {
            Ok(Some(summary)) => json!({
                "schema": "mister-magik-launch-return-pprof-v1",
                "state": "complete",
                "duration_secs": summary.duration_secs,
                "hz": summary.hz,
                "sample_stacks": summary.sample_stacks,
                "sample_hits": summary.sample_hits,
                "stackless_sample_hits": summary.stackless_sample_hits,
                "out_path": summary.out_path,
                "bytes": summary.bytes,
            }),
            Ok(None) => json!({
                "schema": "mister-magik-launch-return-pprof-v1",
                "state": "failed",
                "error": "profiler-produced-no-summary",
            }),
            Err(error) => json!({
                "schema": "mister-magik-launch-return-pprof-v1",
                "state": "failed",
                "error": error,
            }),
        };
        let path = complete_path.ok_or_else(|| "MISTER_PPROF_COMPLETE is missing".to_string())?;
        if let Some(parent) = std::path::Path::new(&path).parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(path, format!("{metadata}\n")).map_err(|error| error.to_string())?;
        result
    }

    pub fn finish_launch_return_async(
        profiler: Option<CpuProfiler>,
        config: &CpuProfileConfig,
    ) -> Result<(), String> {
        let Some(profiler) = profiler else {
            return Ok(());
        };
        let complete_path = config.complete_path.clone();
        std::thread::Builder::new()
            .name("launch-return-profile".into())
            .spawn(move || {
                if let Err(error) = finish_launch_return(Some(profiler), complete_path.as_deref()) {
                    crate::ui_errln!("launch-return cpu profile failed: {error}");
                }
            })
            .map(|_| ())
            .map_err(|error| format!("launch-return cpu profile worker spawn failed: {error}"))
    }

    pub fn finish_cold_boot_async(
        profiler: Option<CpuProfiler>,
        config: &CpuProfileConfig,
    ) -> Result<(), String> {
        let Some(profiler) = profiler else {
            return Ok(());
        };
        let complete_path = config.complete_path.clone();
        std::thread::Builder::new()
            .name("cold-boot-profile".into())
            .spawn(move || {
                let result = finish(Some(profiler));
                let metadata = match &result {
                    Ok(Some(summary)) => json!({
                        "schema": "mister-magik-cold-boot-pprof-v1",
                        "state": "complete",
                        "duration_secs": summary.duration_secs,
                        "hz": summary.hz,
                        "sample_stacks": summary.sample_stacks,
                        "sample_hits": summary.sample_hits,
                        "stackless_sample_hits": summary.stackless_sample_hits,
                        "out_path": summary.out_path,
                        "bytes": summary.bytes,
                    }),
                    Ok(None) => json!({
                        "schema": "mister-magik-cold-boot-pprof-v1",
                        "state": "failed",
                        "error": "profiler-produced-no-summary",
                    }),
                    Err(error) => json!({
                        "schema": "mister-magik-cold-boot-pprof-v1",
                        "state": "failed",
                        "error": error,
                    }),
                };
                if let Some(path) = complete_path {
                    if let Some(parent) = std::path::Path::new(&path).parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::write(path, format!("{metadata}\n"));
                }
                if let Err(error) = result {
                    crate::ui_errln!("cold-boot cpu profile failed: {error}");
                }
            })
            .map(|_| ())
            .map_err(|error| format!("cold-boot cpu profile worker spawn failed: {error}"))
    }

    pub fn finish_system_entry_async(
        profiler: Option<CpuProfiler>,
        config: &CpuProfileConfig,
    ) -> Result<(), String> {
        let Some(profiler) = profiler else {
            return Ok(());
        };
        let complete_path = config.complete_path.clone();
        std::thread::Builder::new()
            .name("system-entry-profile".into())
            .spawn(move || {
                let result = finish(Some(profiler));
                let metadata = match &result {
                    Ok(Some(summary)) => json!({
                        "schema": "mister-magik-system-entry-pprof-v1",
                        "state": "complete",
                        "duration_secs": summary.duration_secs,
                        "hz": summary.hz,
                        "sample_stacks": summary.sample_stacks,
                        "sample_hits": summary.sample_hits,
                        "stackless_sample_hits": summary.stackless_sample_hits,
                        "out_path": summary.out_path,
                        "bytes": summary.bytes,
                    }),
                    Ok(None) => json!({
                        "schema": "mister-magik-system-entry-pprof-v1",
                        "state": "failed",
                        "error": "profiler-produced-no-summary",
                    }),
                    Err(error) => json!({
                        "schema": "mister-magik-system-entry-pprof-v1",
                        "state": "failed",
                        "error": error,
                    }),
                };
                if let Some(path) = complete_path {
                    if let Some(parent) = std::path::Path::new(&path).parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::write(path, format!("{metadata}\n"));
                }
                if let Err(error) = result {
                    crate::ui_errln!("system-entry cpu profile failed: {error}");
                }
            })
            .map(|_| ())
            .map_err(|error| format!("system-entry profile worker spawn failed: {error}"))
    }

    fn write_folded_report(report: &pprof::Report, path: &str) -> Result<u64, String> {
        use std::fmt::Write as _;
        use std::io::Write as _;

        let mut lines: Vec<String> = report
            .data
            .iter()
            .map(|(key, value)| {
                let mut line = key.thread_name_or_id();
                line.push(';');
                for frame in key.frames.iter().rev() {
                    for symbol in frame.iter().rev() {
                        write!(&mut line, "{};", symbol)
                            .expect("writing folded stack line to String cannot fail");
                    }
                }
                line.pop();
                write!(&mut line, " {}", value)
                    .expect("writing folded stack count to String cannot fail");
                line
            })
            .collect();
        lines.sort();

        let mut file = std::fs::File::create(path)
            .map_err(|e| format!("cpu_profile: create folded stack file {path} failed: {e}"))?;
        for line in lines {
            writeln!(file, "{line}")
                .map_err(|e| format!("cpu_profile: write folded stack file {path} failed: {e}"))?;
        }
        let bytes = file
            .metadata()
            .map(|m| m.len())
            .map_err(|e| format!("cpu_profile: stat folded stack file {path} failed: {e}"))?;
        crate::ui_logln!("cpu_profile: wrote folded stacks to {path} ({bytes} bytes)");
        Ok(bytes)
    }

    struct CatalogBuildSession {
        profiler: Option<CpuProfiler>,
        operation: Option<String>,
        finished: bool,
        scope: &'static str,
    }

    pub struct CatalogBuildProfiler {
        config: CpuProfileConfig,
        session: std::sync::Arc<std::sync::Mutex<CatalogBuildSession>>,
    }

    impl CatalogBuildProfiler {
        pub fn capture_process() -> Self {
            let values: std::collections::BTreeMap<String, String> = std::env::vars().collect();
            let config =
                CpuProfileConfig::capture_with(|name| values.get(name).map(String::as_str));
            let scope = if config.catalog_build_full {
                "full-build"
            } else {
                "post-scan"
            };
            Self {
                config,
                session: std::sync::Arc::new(std::sync::Mutex::new(CatalogBuildSession {
                    profiler: None,
                    operation: None,
                    finished: false,
                    scope,
                })),
            }
        }

        pub fn arm(&mut self, operation: &str) {
            if !self.config.catalog_build_requested() {
                return;
            }
            let Ok(mut session) = self.session.lock() else {
                crate::ui_errln!("catalog-build cpu profile session lock poisoned");
                return;
            };
            if session.operation.is_none() && !session.finished {
                session.operation = Some(operation.to_owned());
            }
            drop(session);
            if self.config.catalog_build_full {
                self.begin(operation);
            }
        }

        pub fn begin(&mut self, operation: &str) {
            if !self.config.catalog_build_requested() {
                return;
            }
            if self.session.lock().map_or(true, |session| {
                session.profiler.is_some()
                    || session.finished
                    || session
                        .operation
                        .as_deref()
                        .is_some_and(|armed| armed != operation)
            }) {
                return;
            }
            let profiler = start_enabled(&self.config);
            {
                let Ok(mut session) = self.session.lock() else {
                    crate::ui_errln!("catalog-build cpu profile session lock poisoned");
                    return;
                };
                if session.profiler.is_some() || session.finished {
                    return;
                }
                if session.operation.is_none() {
                    session.operation = Some(operation.to_owned());
                }
                session.profiler = profiler;
            }
            if self
                .session
                .lock()
                .ok()
                .is_some_and(|session| session.profiler.is_none())
            {
                finish_catalog_build_async(
                    self.session.clone(),
                    self.config.complete_path.clone(),
                    "failed",
                    "profiler-start-failed",
                );
                return;
            }

            let session = self.session.clone();
            let complete_path = self.config.complete_path.clone();
            let timeout = self.config.duration;
            if let Err(error) = std::thread::Builder::new()
                .name("catalog-build-timeout".into())
                .spawn(move || {
                    std::thread::sleep(timeout);
                    finalize_catalog_build(session, complete_path, "failed", "timeout");
                })
            {
                crate::ui_errln!("catalog-build cpu profile watchdog spawn failed: {error}");
                self.fail("watchdog-spawn-failed");
            }
        }

        pub fn persisted(&mut self) {
            self.finish("complete", "persisted");
        }

        pub fn unchanged(&mut self) {
            self.finish("complete", "unchanged");
        }

        pub fn fail(&mut self, reason: &'static str) {
            self.arm("unknown");
            self.finish("failed", reason);
        }

        fn finish(&mut self, state: &'static str, outcome: &'static str) {
            if !self.config.catalog_build_requested() {
                return;
            }
            finish_catalog_build_async(
                self.session.clone(),
                self.config.complete_path.clone(),
                state,
                outcome,
            );
        }
    }

    fn finish_catalog_build_async(
        session: std::sync::Arc<std::sync::Mutex<CatalogBuildSession>>,
        complete_path: Option<String>,
        state: &'static str,
        outcome: &'static str,
    ) {
        if let Err(error) = std::thread::Builder::new()
            .name("catalog-build-profile".into())
            .spawn(move || finalize_catalog_build(session, complete_path, state, outcome))
        {
            crate::ui_errln!("catalog-build cpu profile finalizer spawn failed: {error}");
        }
    }

    fn finalize_catalog_build(
        session: std::sync::Arc<std::sync::Mutex<CatalogBuildSession>>,
        complete_path: Option<String>,
        state: &str,
        outcome: &str,
    ) {
        let (profiler, operation, scope) = {
            let Ok(mut session) = session.lock() else {
                crate::ui_errln!("catalog-build cpu profile session lock poisoned");
                return;
            };
            if session.finished || session.operation.is_none() {
                return;
            }
            session.finished = true;
            (
                session.profiler.take(),
                session.operation.take(),
                session.scope,
            )
        };
        let result = finish(profiler);
        let mut metadata = match &result {
            Ok(Some(summary)) => json!({
                "schema": "mister-magik-catalog-build-pprof-v1",
                "scope": scope,
                "state": state,
                "outcome": outcome,
                "duration_secs": summary.duration_secs,
                "hz": summary.hz,
                "sample_stacks": summary.sample_stacks,
                "sample_hits": summary.sample_hits,
                "stackless_sample_hits": summary.stackless_sample_hits,
                "out_path": summary.out_path,
                "bytes": summary.bytes,
            }),
            Ok(None) => json!({
                "schema": "mister-magik-catalog-build-pprof-v1",
                "scope": scope,
                "state": "failed",
                "outcome": outcome,
                "error": "profiler-produced-no-summary",
            }),
            Err(error) => json!({
                "schema": "mister-magik-catalog-build-pprof-v1",
                "scope": scope,
                "state": "failed",
                "outcome": outcome,
                "error": error,
            }),
        };
        metadata["operation"] = json!(operation);
        match complete_path {
            Some(path) => {
                if let Some(parent) = std::path::Path::new(&path).parent()
                    && let Err(error) = fs::create_dir_all(parent)
                {
                    crate::ui_errln!("catalog-build completion directory failed: {error}");
                    return;
                }
                if let Err(error) = fs::write(path, format!("{metadata}\n")) {
                    crate::ui_errln!("catalog-build completion write failed: {error}");
                }
            }
            None => crate::ui_errln!("catalog-build cpu profile missing MISTER_PPROF_COMPLETE"),
        }
        if let Err(error) = result {
            crate::ui_errln!("catalog-build cpu profile failed: {error}");
        }
    }

    enum State {
        Disabled,
        Waiting,
        Warming {
            started: Instant,
        },
        Active {
            profiler: CpuProfiler,
            started: Instant,
            first_frame: u64,
        },
        Finalizing,
        Failed,
    }

    pub struct ScreensaverProfiler {
        state: State,
        config: CpuProfileConfig,
        trigger: Option<BoundedProfileTrigger>,
        warmup: Duration,
        duration: Duration,
        complete_path: Option<String>,
    }

    impl ScreensaverProfiler {
        pub fn from_config(config: &CpuProfileConfig) -> Self {
            let trigger = config.trigger;
            let state = if matches!(
                trigger,
                Some(
                    BoundedProfileTrigger::Screensaver
                        | BoundedProfileTrigger::NavigationTransitions
                        | BoundedProfileTrigger::SettingsNavigationTransitions
                        | BoundedProfileTrigger::OrientationTransitionFade
                        | BoundedProfileTrigger::OrientationTransitionZoom
                        | BoundedProfileTrigger::LauncherResponse
                        | BoundedProfileTrigger::ArcadeVelocityScroll
                )
            ) {
                set_screensaver_profile_state(ScreensaverProfileState::Waiting);
                State::Waiting
            } else {
                set_screensaver_profile_state(ScreensaverProfileState::Disabled);
                State::Disabled
            };
            Self {
                state,
                config: config.clone(),
                trigger,
                warmup: config.warmup,
                duration: config.duration,
                complete_path: config.complete_path.clone(),
            }
        }

        pub fn begin_screensaver(&mut self, first_frame: u64) {
            self.begin(BoundedProfileTrigger::Screensaver, first_frame);
        }

        pub fn begin_navigation_transition(&mut self, first_frame: u64) {
            self.begin(BoundedProfileTrigger::NavigationTransitions, first_frame);
        }

        pub fn begin_settings_navigation_transition(&mut self, first_frame: u64) {
            self.begin(
                BoundedProfileTrigger::SettingsNavigationTransitions,
                first_frame,
            );
        }

        pub fn complete_settings_navigation_transitions(&mut self, next_frame: u64) {
            if self.trigger == Some(BoundedProfileTrigger::SettingsNavigationTransitions) {
                self.complete(next_frame, false);
            }
        }

        pub fn begin_orientation_transitions(&mut self, first_frame: u64) {
            if let Some(
                trigger @ (BoundedProfileTrigger::OrientationTransitionFade
                | BoundedProfileTrigger::OrientationTransitionZoom),
            ) = self.trigger
            {
                self.begin(trigger, first_frame);
            }
        }

        pub fn complete_orientation_transitions(&mut self, next_frame: u64) {
            if matches!(
                self.trigger,
                Some(
                    BoundedProfileTrigger::OrientationTransitionFade
                        | BoundedProfileTrigger::OrientationTransitionZoom
                )
            ) {
                self.complete(next_frame, true);
            }
        }

        pub fn begin_launcher_response(&mut self, first_frame: u64) {
            self.begin(BoundedProfileTrigger::LauncherResponse, first_frame);
        }

        pub fn begin_arcade_velocity_scroll(&mut self, first_frame: u64) {
            self.begin(BoundedProfileTrigger::ArcadeVelocityScroll, first_frame);
        }

        pub fn complete_arcade_velocity_scroll(&mut self, next_frame: u64) {
            if self.trigger == Some(BoundedProfileTrigger::ArcadeVelocityScroll) {
                self.complete(next_frame, false);
            }
        }

        pub fn complete_launcher_response(&mut self, next_frame: u64) {
            if self.trigger == Some(BoundedProfileTrigger::LauncherResponse) {
                self.complete(next_frame, false);
            }
        }

        fn begin(&mut self, trigger: BoundedProfileTrigger, first_frame: u64) {
            if self.trigger != Some(trigger) || !matches!(self.state, State::Waiting) {
                return;
            }
            if !self.warmup.is_zero() {
                self.state = State::Warming {
                    started: Instant::now(),
                };
                return;
            }
            self.start(first_frame);
        }

        fn start(&mut self, first_frame: u64) {
            match start_enabled(&self.config) {
                Some(profiler) => {
                    self.state = State::Active {
                        profiler,
                        started: Instant::now(),
                        first_frame,
                    };
                    set_screensaver_profile_state(ScreensaverProfileState::Active);
                }
                None => {
                    self.fail("profiler-start-failed");
                }
            }
        }

        pub fn poll(&mut self, next_frame: u64) {
            let warmed = matches!(
                &self.state,
                State::Warming { started } if started.elapsed() >= self.warmup
            );
            if warmed {
                self.start(next_frame);
            }
            if matches!(
                self.trigger,
                Some(
                    BoundedProfileTrigger::OrientationTransitionFade
                        | BoundedProfileTrigger::OrientationTransitionZoom
                )
            ) {
                return;
            }
            let elapsed = match &self.state {
                State::Active { started, .. } => started.elapsed(),
                _ => return,
            };
            if elapsed < self.duration {
                return;
            }
            self.complete(next_frame, false);
        }

        fn complete(&mut self, next_frame: u64, include_orientation_route: bool) {
            let state = std::mem::replace(&mut self.state, State::Failed);
            let State::Active {
                profiler,
                first_frame,
                ..
            } = state
            else {
                return;
            };
            let (first_frame, last_frame) =
                screensaver_profile_frame_bounds(first_frame, next_frame);
            let trigger = self
                .trigger
                .expect("active bounded profile must retain its trigger");
            let complete_path = self.complete_path.clone();
            let worker = std::thread::Builder::new()
                .name("bounded-profile".into())
                .spawn(move || {
                    let result = finish(Some(profiler));
                    let metadata = match &result {
                        Ok(Some(summary)) => json!({
                            "schema": trigger.schema(),
                            "trigger": trigger.label(),
                            "state": "complete",
                            "duration_secs": summary.duration_secs,
                            "hz": summary.hz,
                            "sample_stacks": summary.sample_stacks,
                            "sample_hits": summary.sample_hits,
                            "stackless_sample_hits": summary.stackless_sample_hits,
                            "out_path": summary.out_path,
                            "bytes": summary.bytes,
                            "first_frame": first_frame,
                            "last_frame": last_frame,
                            "orientations": (trigger == BoundedProfileTrigger::SettingsNavigationTransitions)
                                .then_some(["normal", "monitor-counterclockwise"]),
                            "route": include_orientation_route.then_some([
                                "normal",
                                "monitor-clockwise",
                                "monitor-counterclockwise",
                                "normal",
                                "monitor-counterclockwise",
                                "monitor-clockwise",
                                "normal",
                            ]),
                            "effects": include_orientation_route.then_some([
                                "brightness-fade",
                                "center-pixel-zoom",
                            ]),
                        }),
                        Ok(None) => json!({
                            "schema": trigger.schema(),
                            "trigger": trigger.label(),
                            "state": "failed",
                            "error": "profiler-produced-no-summary",
                        }),
                        Err(error) => json!({
                            "schema": trigger.schema(),
                            "trigger": trigger.label(),
                            "state": "failed",
                            "error": error,
                        }),
                    };
                    let completion = write_bounded_completion(
                        complete_path.as_deref(),
                        &metadata.to_string(),
                    );
                    match (result, completion) {
                        (Ok(Some(_)), Ok(())) => {
                            set_screensaver_profile_state(ScreensaverProfileState::Complete);
                        }
                        (result, completion) => {
                            if let Err(error) = result {
                                crate::ui_errln!("bounded cpu profile failed: {error}");
                            }
                            if let Err(error) = completion {
                                crate::ui_errln!(
                                    "bounded cpu profile completion write failed: {error}"
                                );
                            }
                            set_screensaver_profile_state(ScreensaverProfileState::Failed);
                        }
                    }
                });
            match worker {
                Ok(_) => self.state = State::Finalizing,
                Err(error) => self.fail(&format!("profile-worker-spawn-failed:{error}")),
            }
        }

        fn fail(&mut self, error: &str) {
            let trigger = self.trigger.unwrap_or(BoundedProfileTrigger::Screensaver);
            let metadata = json!({
                "schema": trigger.schema(),
                "trigger": trigger.label(),
                "state": "failed",
                "error": error,
            });
            let _ = write_bounded_completion(self.complete_path.as_deref(), &metadata.to_string());
            self.state = State::Failed;
            set_screensaver_profile_state(ScreensaverProfileState::Failed);
            crate::ui_errln!("screensaver cpu profile failed: {error}");
        }
    }

    fn write_bounded_completion(path: Option<&str>, text: &str) -> Result<(), String> {
        let Some(path) = path else {
            return Err("MISTER_PPROF_COMPLETE is missing".into());
        };
        if let Some(parent) = std::path::Path::new(path).parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(path, format!("{text}\n")).map_err(|error| error.to_string())
    }
}

#[cfg(feature = "profile")]
pub use imp::{
    CatalogBuildProfiler, CpuProfiler, ScreensaverProfiler, finish, finish_cold_boot_async,
    finish_launch_return_async, finish_system_entry_async, start, start_process_entry,
    start_system_entry,
};

#[cfg(not(feature = "profile"))]
mod stub {
    use super::{
        BoundedProfileTrigger, CpuProfileConfig, CpuProfileSummary, ScreensaverProfileState,
        set_screensaver_profile_state,
    };

    pub struct CpuProfiler;

    pub fn start(config: &CpuProfileConfig) -> Option<CpuProfiler> {
        if config.enabled {
            crate::ui_errln!(
                "cpu_profile: MISTER_PPROF=1 ignored — the runtime lacks the `profile` feature; \
                 install the canonical device runtime with `scripts/agent deliver`"
            );
        }
        None
    }

    pub fn start_process_entry(config: &CpuProfileConfig) -> Option<CpuProfiler> {
        if matches!(
            config.trigger,
            Some(BoundedProfileTrigger::LaunchReturn | BoundedProfileTrigger::ColdBoot)
        ) {
            set_screensaver_profile_state(ScreensaverProfileState::Failed);
        }
        None
    }

    pub fn start_system_entry(_config: &CpuProfileConfig) -> Option<CpuProfiler> {
        None
    }

    pub fn finish(_: Option<CpuProfiler>) -> Result<Option<CpuProfileSummary>, String> {
        Ok(None)
    }

    pub fn finish_launch_return(
        profiler: Option<CpuProfiler>,
        _complete_path: Option<&str>,
    ) -> Result<Option<CpuProfileSummary>, String> {
        finish(profiler)
    }

    pub fn finish_launch_return_async(
        profiler: Option<CpuProfiler>,
        config: &CpuProfileConfig,
    ) -> Result<(), String> {
        let Some(profiler) = profiler else {
            return Ok(());
        };
        finish_launch_return(Some(profiler), config.complete_path.as_deref()).map(|_| ())
    }

    pub fn finish_cold_boot_async(
        profiler: Option<CpuProfiler>,
        _config: &CpuProfileConfig,
    ) -> Result<(), String> {
        let Some(profiler) = profiler else {
            return Ok(());
        };
        finish(Some(profiler)).map(|_| ())
    }

    pub fn finish_system_entry_async(
        profiler: Option<CpuProfiler>,
        _config: &CpuProfileConfig,
    ) -> Result<(), String> {
        let Some(profiler) = profiler else {
            return Ok(());
        };
        finish(Some(profiler)).map(|_| ())
    }

    pub struct ScreensaverProfiler;

    impl ScreensaverProfiler {
        pub fn from_config(config: &CpuProfileConfig) -> Self {
            set_screensaver_profile_state(if config.trigger.is_some() {
                ScreensaverProfileState::Failed
            } else {
                ScreensaverProfileState::Disabled
            });
            Self
        }

        pub fn begin_screensaver(&mut self, _first_frame: u64) {}

        pub fn begin_navigation_transition(&mut self, _first_frame: u64) {}

        pub fn begin_settings_navigation_transition(&mut self, _first_frame: u64) {}

        pub fn complete_settings_navigation_transitions(&mut self, _next_frame: u64) {}

        pub fn begin_orientation_transitions(&mut self, _first_frame: u64) {}

        pub fn complete_orientation_transitions(&mut self, _next_frame: u64) {}

        pub fn begin_launcher_response(&mut self, _first_frame: u64) {}

        pub fn begin_arcade_velocity_scroll(&mut self, _first_frame: u64) {}

        pub fn complete_arcade_velocity_scroll(&mut self, _next_frame: u64) {}

        pub fn complete_launcher_response(&mut self, _next_frame: u64) {}

        pub fn poll(&mut self, _next_frame: u64) {}
    }

    pub struct CatalogBuildProfiler;

    impl CatalogBuildProfiler {
        pub fn capture_process() -> Self {
            Self
        }

        pub fn arm(&mut self, _operation: &str) {}

        pub fn begin(&mut self, _operation: &str) {}

        pub fn persisted(&mut self) {}

        pub fn unchanged(&mut self) {}

        pub fn fail(&mut self, _reason: &'static str) {}
    }
}

#[cfg(not(feature = "profile"))]
pub use stub::{
    CatalogBuildProfiler, CpuProfiler, ScreensaverProfiler, finish, finish_cold_boot_async,
    finish_launch_return_async, finish_system_entry_async, start, start_process_entry,
    start_system_entry,
};

#[cfg(feature = "diagnostics")]
pub fn start_from_env() -> Option<CpuProfiler> {
    let values: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let config = CpuProfileConfig::capture_with(|name| values.get(name).map(String::as_str));
    start(&config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screensaver_profile_duration_is_bounded() {
        assert_eq!(
            screensaver_profile_duration_from_value(Some("0")),
            Duration::from_secs(1)
        );
        assert_eq!(
            screensaver_profile_duration_from_value(Some("999")),
            Duration::from_secs(300)
        );
        assert_eq!(
            screensaver_profile_duration_from_value(None),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn screensaver_profile_warmup_is_optional_and_bounded() {
        assert_eq!(
            screensaver_profile_warmup_from_value(Some("30")),
            Duration::from_secs(30)
        );
        assert_eq!(
            screensaver_profile_warmup_from_value(Some("999")),
            Duration::from_secs(300)
        );
        assert_eq!(screensaver_profile_warmup_from_value(None), Duration::ZERO);
    }

    #[test]
    fn catalog_build_profile_timeout_covers_whole_card_builds_and_is_bounded() {
        assert_eq!(
            catalog_build_profile_timeout_from_value(None),
            Duration::from_secs(600)
        );
        assert_eq!(
            catalog_build_profile_timeout_from_value(Some("9999")),
            Duration::from_secs(1_200)
        );
    }

    #[test]
    fn screensaver_profile_frame_bounds_are_inclusive() {
        assert_eq!(screensaver_profile_frame_bounds(475, 2_240), (475, 2_239));
        assert_eq!(screensaver_profile_frame_bounds(475, 475), (475, 475));
    }

    #[test]
    fn bounded_profile_trigger_accepts_only_owned_triggers() {
        assert_eq!(
            bounded_profile_trigger_from_values(Some("1"), Some("screensaver")),
            Some(BoundedProfileTrigger::Screensaver)
        );
        assert_eq!(
            bounded_profile_trigger_from_values(Some("1"), Some("navigation-transitions")),
            Some(BoundedProfileTrigger::NavigationTransitions)
        );
        assert_eq!(
            bounded_profile_trigger_from_values(Some("1"), Some("settings-navigation-transitions")),
            Some(BoundedProfileTrigger::SettingsNavigationTransitions)
        );
        assert_eq!(
            bounded_profile_trigger_from_values(Some("1"), Some("launch-return")),
            Some(BoundedProfileTrigger::LaunchReturn)
        );
        assert_eq!(
            bounded_profile_trigger_from_values(Some("1"), Some("cold-boot")),
            Some(BoundedProfileTrigger::ColdBoot)
        );
        assert_eq!(
            bounded_profile_trigger_from_values(Some("1"), Some("orientation-transition-fade")),
            Some(BoundedProfileTrigger::OrientationTransitionFade)
        );
        assert_eq!(
            bounded_profile_trigger_from_values(Some("1"), Some("orientation-transition-zoom")),
            Some(BoundedProfileTrigger::OrientationTransitionZoom)
        );
        assert_eq!(
            bounded_profile_trigger_from_values(Some("1"), Some("launcher-response")),
            Some(BoundedProfileTrigger::LauncherResponse)
        );
        assert_eq!(
            bounded_profile_trigger_from_values(Some("1"), Some("arcade-velocity-scroll")),
            Some(BoundedProfileTrigger::ArcadeVelocityScroll)
        );
        assert_eq!(
            bounded_profile_trigger_from_values(Some("1"), Some("catalog-build")),
            Some(BoundedProfileTrigger::CatalogBuild)
        );
        assert_eq!(
            bounded_profile_trigger_from_values(Some("1"), Some("catalog-build-full")),
            Some(BoundedProfileTrigger::CatalogBuild)
        );
        assert_eq!(
            bounded_profile_trigger_from_values(Some("0"), Some("navigation-transitions")),
            None
        );
        assert_eq!(
            bounded_profile_trigger_from_values(Some("1"), Some("unowned")),
            None
        );
    }

    #[test]
    fn system_entry_profile_requires_its_exact_trigger() {
        assert!(system_entry_profile_requested_from_values(
            Some("1"),
            Some("system-entry")
        ));
        assert!(!system_entry_profile_requested_from_values(
            Some("0"),
            Some("system-entry")
        ));
        assert!(!system_entry_profile_requested_from_values(
            Some("1"),
            Some("screensaver")
        ));
    }

    #[test]
    fn cpu_profile_config_preserves_captured_outputs_and_modes() {
        let values = std::collections::BTreeMap::from([
            (PPROF, "1"),
            (PPROF_TRIGGER, "launch-return"),
            (PPROF_HZ, "999"),
            (PPROF_OUT, "/tmp/profile.svg"),
            (PPROF_COMPLETE, "/tmp/profile-complete.json"),
        ]);
        let config = CpuProfileConfig::capture_with(|name| values.get(name).copied());

        assert!(config.launch_return_requested());
        assert!(!config.cold_boot_requested());
        assert!(!config.cold_boot_catalog_requested());
        assert_eq!(config.hz, 999);
        assert_eq!(config.out_path, "/tmp/profile.svg");
        assert_eq!(
            config.complete_path.as_deref(),
            Some("/tmp/profile-complete.json")
        );
    }

    #[test]
    fn cold_boot_catalog_profile_runs_until_catalog_ready() {
        let values = std::collections::BTreeMap::from([
            (PPROF, "1"),
            (PPROF_TRIGGER, COLD_BOOT_CATALOG_TRIGGER),
        ]);
        let config = CpuProfileConfig::capture_with(|name| values.get(name).copied());
        assert!(config.cold_boot_requested());
        assert!(config.cold_boot_catalog_requested());
    }
}
