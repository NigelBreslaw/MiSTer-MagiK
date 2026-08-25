// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::device::DeviceClient;
use crate::error::AgentResult;
use crate::model::{
    ArcadeVelocityScrollArm, ArcadeVelocityScrollRoute, ArcadeVelocityScrollRunSpec,
    BenchmarkScenario, Outcome,
};
use crate::progress::{EventKind, Reporter};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn execute(
    repository: &Path,
    scenario: BenchmarkScenario,
    arm: Option<ArcadeVelocityScrollArm>,
    route: ArcadeVelocityScrollRoute,
    duration_seconds: u64,
    fresh_catalog: bool,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    require_clean_installed_commit(
        repository,
        scenario,
        arm,
        route,
        duration_seconds,
        fresh_catalog,
        reporter,
    )
}

trait BenchmarkDevice {
    fn connect(&mut self) -> AgentResult<()>;
    fn verify_development_platform(&mut self) -> AgentResult<()>;
    fn read_active_runtime(&mut self) -> AgentResult<crate::host::ActiveRuntime>;
    fn verify_health(&mut self) -> AgentResult<()>;
    fn read_development_manifest(&mut self) -> AgentResult<String>;
    fn profile(&mut self, profile: BenchmarkProfile, output_dir: PathBuf) -> AgentResult<String>;
    fn profile_arcade_velocity_scroll(
        &mut self,
        spec: ArcadeVelocityScrollRunSpec,
        output_dir: PathBuf,
    ) -> AgentResult<String>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BenchmarkProfile {
    Screensaver,
    MediaPackPersistence,
    RomIdentityHashing,
    PreviewWorkAttribution,
    Search,
    SearchUi,
    CatalogLifecycle,
    CatalogBuildRebuild,
    CatalogResumeValidation,
    CatalogFullBuildRebuild,
    CatalogCorpusInventory,
    CatalogAttributionControl,
    CatalogAttributionPprof,
    CatalogAttributionPmu,
    CatalogAttributionStorage,
    CatalogAttributionFunctionGraph,
    CatalogAttributionStreamline,
    CatalogAttributionReport,
    SystemEntry,
    SystemEntryCritical,
    SystemEntryCriticalConfirm,
    SystemEntryCriticalProfile,
    SystemEntryCriticalStreamline,
    SystemEntryQualification,
    LaunchReturn,
    LaunchReturnOnce,
    LaunchReturnFallback,
    LaunchReturnAttribution,
    ModalInput,
    InputIntegrity,
    LauncherResponse,
    LauncherResponseRetained,
    LauncherResponseAttribution,
    GuiFrameAttribution,
    SettledComposition,
    BridgeModelChurn,
    BridgeModelChurnRetained,
    SchedulerTrace,
    StorageAttribution,
    ArcadeVelocityScroll,
    ArcadeVelocityScrollAttribution,
    TransitionStreamline,
    AgentObserverAttribution,
    AgentIoAttribution,
    InputLatencyLab,
    LauncherResponseStreamline,
    ColdBoot { pprof: bool, fresh_catalog: bool },
    NavigationTransitions,
    SettingsNavigation,
    SettingsNavigationPprof,
    OrientationTransitionFade,
    OrientationTransitionZoom,
    OrientationTransitionFadePprof,
    OrientationTransitionZoomPprof,
    NeonAttribution,
    Pmu,
    Streamline,
}

impl BenchmarkDevice for DeviceClient {
    fn connect(&mut self) -> AgentResult<()> {
        self.read(crate::NativeDevice::discover)
    }

    fn verify_development_platform(&mut self) -> AgentResult<()> {
        self.read(crate::NativeDevice::verify_development_platform)
    }

    fn read_active_runtime(&mut self) -> AgentResult<crate::host::ActiveRuntime> {
        self.read(crate::NativeDevice::read_active_runtime)
    }

    fn verify_health(&mut self) -> AgentResult<()> {
        self.read(crate::NativeDevice::verify_development_health)
    }

    fn read_development_manifest(&mut self) -> AgentResult<String> {
        self.read(crate::NativeDevice::read_development_manifest)
    }

    fn profile(&mut self, profile: BenchmarkProfile, output_dir: PathBuf) -> AgentResult<String> {
        self.mutate(|device| match profile {
            BenchmarkProfile::Screensaver => device.profile_screensaver(&output_dir),
            BenchmarkProfile::MediaPackPersistence => {
                device.profile_media_pack_persistence(&output_dir)
            }
            BenchmarkProfile::RomIdentityHashing => {
                device.profile_rom_identity_hashing(&output_dir)
            }
            BenchmarkProfile::PreviewWorkAttribution => {
                device.profile_preview_work_attribution(&output_dir)
            }
            BenchmarkProfile::Search => device.profile_search(&output_dir),
            BenchmarkProfile::SearchUi => device.verify_search_ui(&output_dir),
            BenchmarkProfile::CatalogLifecycle => device.profile_catalog_lifecycle(&output_dir),
            BenchmarkProfile::CatalogBuildRebuild => {
                device.profile_catalog_build_rebuild(&output_dir)
            }
            BenchmarkProfile::CatalogResumeValidation => {
                device.profile_catalog_resume_validation(&output_dir)
            }
            BenchmarkProfile::CatalogFullBuildRebuild => {
                device.profile_catalog_full_build_rebuild(&output_dir)
            }
            BenchmarkProfile::CatalogCorpusInventory => {
                device.profile_catalog_corpus_inventory(&output_dir)
            }
            BenchmarkProfile::CatalogAttributionControl => {
                device.profile_catalog_attribution_control(&output_dir)
            }
            BenchmarkProfile::CatalogAttributionPprof => {
                device.profile_catalog_attribution_pprof(&output_dir)
            }
            BenchmarkProfile::CatalogAttributionPmu => {
                device.profile_catalog_attribution_pmu(&output_dir)
            }
            BenchmarkProfile::CatalogAttributionStorage => {
                device.profile_catalog_attribution_storage(&output_dir)
            }
            BenchmarkProfile::CatalogAttributionFunctionGraph => {
                device.profile_catalog_attribution_function_graph(&output_dir)
            }
            BenchmarkProfile::CatalogAttributionStreamline => {
                device.profile_catalog_attribution_streamline(&output_dir)
            }
            BenchmarkProfile::CatalogAttributionReport => {
                device.profile_catalog_attribution_report(&output_dir)
            }
            BenchmarkProfile::SystemEntry => device.profile_system_entry(&output_dir),
            BenchmarkProfile::SystemEntryCritical => {
                device.profile_system_entry_critical(&output_dir)
            }
            BenchmarkProfile::SystemEntryCriticalConfirm => {
                device.profile_system_entry_critical_confirm(&output_dir)
            }
            BenchmarkProfile::SystemEntryCriticalProfile => {
                device.profile_system_entry_critical_profile(&output_dir)
            }
            BenchmarkProfile::SystemEntryCriticalStreamline => {
                device.profile_system_entry_critical_streamline(&output_dir)
            }
            BenchmarkProfile::SystemEntryQualification => {
                device.profile_system_entry_qualification(&output_dir)
            }
            BenchmarkProfile::LaunchReturn => device.profile_launch_return(&output_dir, false),
            BenchmarkProfile::LaunchReturnOnce => device.profile_launch_return_once(&output_dir),
            BenchmarkProfile::LaunchReturnFallback => {
                device.profile_launch_return(&output_dir, true)
            }
            BenchmarkProfile::LaunchReturnAttribution => {
                device.profile_launch_return_attribution(&output_dir)
            }
            BenchmarkProfile::ModalInput => device.verify_modal_input(&output_dir),
            BenchmarkProfile::InputIntegrity => device.verify_input_integrity(&output_dir),
            BenchmarkProfile::LauncherResponse => device.verify_launcher_response(&output_dir),
            BenchmarkProfile::LauncherResponseRetained => {
                device.verify_launcher_response_retained(&output_dir)
            }
            BenchmarkProfile::LauncherResponseAttribution => {
                device.profile_launcher_response_attribution(&output_dir)
            }
            BenchmarkProfile::GuiFrameAttribution => {
                device.profile_gui_frame_attribution(&output_dir)
            }
            BenchmarkProfile::SettledComposition => device.profile_settled_composition(&output_dir),
            BenchmarkProfile::BridgeModelChurn => device.profile_bridge_model_churn(&output_dir),
            BenchmarkProfile::BridgeModelChurnRetained => {
                device.profile_bridge_model_churn_retained(&output_dir)
            }
            BenchmarkProfile::SchedulerTrace => device.profile_scheduler_trace(&output_dir),
            BenchmarkProfile::StorageAttribution => device.profile_storage_attribution(&output_dir),
            BenchmarkProfile::ArcadeVelocityScroll => {
                device.profile_arcade_velocity_scroll(&output_dir)
            }
            BenchmarkProfile::ArcadeVelocityScrollAttribution => {
                device.profile_arcade_velocity_scroll_attribution(&output_dir)
            }
            BenchmarkProfile::TransitionStreamline => {
                device.profile_transition_streamline(&output_dir)
            }
            BenchmarkProfile::AgentObserverAttribution => {
                device.profile_agent_observer_attribution(&output_dir)
            }
            BenchmarkProfile::AgentIoAttribution => {
                device.profile_agent_io_attribution(&output_dir)
            }
            BenchmarkProfile::InputLatencyLab => device.verify_input_latency_lab(&output_dir),
            BenchmarkProfile::LauncherResponseStreamline => {
                device.profile_launcher_response_streamline(&output_dir)
            }
            BenchmarkProfile::ColdBoot {
                pprof,
                fresh_catalog,
            } => device.profile_cold_boot(&output_dir, pprof, fresh_catalog),
            BenchmarkProfile::NavigationTransitions => {
                device.profile_navigation_transitions(&output_dir)
            }
            BenchmarkProfile::SettingsNavigation => {
                device.profile_settings_navigation(&output_dir, false)
            }
            BenchmarkProfile::SettingsNavigationPprof => {
                device.profile_settings_navigation(&output_dir, true)
            }
            BenchmarkProfile::OrientationTransitionFade => {
                device.profile_orientation_transition(&output_dir, "brightness-fade", false)
            }
            BenchmarkProfile::OrientationTransitionZoom => {
                device.profile_orientation_transition(&output_dir, "center-pixel-zoom", false)
            }
            BenchmarkProfile::OrientationTransitionFadePprof => {
                device.profile_orientation_transition(&output_dir, "brightness-fade", true)
            }
            BenchmarkProfile::OrientationTransitionZoomPprof => {
                device.profile_orientation_transition(&output_dir, "center-pixel-zoom", true)
            }
            BenchmarkProfile::NeonAttribution => device.profile_neon_attribution(&output_dir),
            BenchmarkProfile::Pmu => device.profile_pmu(&output_dir),
            BenchmarkProfile::Streamline => device.profile_streamline(&output_dir),
        })
    }

    fn profile_arcade_velocity_scroll(
        &mut self,
        spec: ArcadeVelocityScrollRunSpec,
        output_dir: PathBuf,
    ) -> AgentResult<String> {
        self.mutate(|device| device.profile_arcade_velocity_scroll_run(&output_dir, spec))
    }
}

fn require_clean_installed_commit(
    repository: &Path,
    scenario: BenchmarkScenario,
    arm: Option<ArcadeVelocityScrollArm>,
    route: ArcadeVelocityScrollRoute,
    duration_seconds: u64,
    fresh_catalog: bool,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    if arm.is_some() && scenario != BenchmarkScenario::ArcadeVelocityScrollAttribution {
        return Err(
            "benchmark arm is supported only for arcade-velocity-scroll-attribution".into(),
        );
    }
    if route != ArcadeVelocityScrollRoute::Active
        && scenario != BenchmarkScenario::ArcadeVelocityScrollAttribution
    {
        return Err(
            "benchmark route is supported only for arcade-velocity-scroll-attribution".into(),
        );
    }
    if route != ArcadeVelocityScrollRoute::Active && arm.is_none() {
        return Err("an explicit Arcade benchmark route requires one profiler arm".into());
    }
    if duration_seconds != 40 && scenario != BenchmarkScenario::ArcadeVelocityScrollAttribution {
        return Err(
            "--duration-seconds is supported only for arcade-velocity-scroll-attribution".into(),
        );
    }
    if fresh_catalog
        && !matches!(
            scenario,
            BenchmarkScenario::ColdBoot | BenchmarkScenario::ColdBootPprof
        )
    {
        return Err("--fresh-catalog is supported only for cold-boot benchmarks".into());
    }
    let head = crate::git::value(repository, &["rev-parse", "HEAD"])?;
    if !crate::git::value(repository, &["status", "--porcelain"])?.is_empty() {
        return Err("benchmark requires a clean exact-commit worktree".into());
    }
    if let Some(command) = particle_scene_lab_command(scenario) {
        reporter.emit(EventKind::Warning, "scene-lab-required", command, Some(100))?;
        return Ok(Outcome::ExternalRequired);
    }
    let mut device = DeviceClient::default();
    reporter.emit(
        EventKind::Progress,
        "preflight",
        &format!("benchmark {} installed-runtime preflight", scenario.label()),
        Some(10),
    )?;
    device.connect()?;
    device.verify_development_platform()?;
    require_active_development_runtime(&device.read_active_runtime()?)?;
    if benchmark_requires_initial_health(scenario) {
        device.verify_health()?;
    }
    let manifest = device.read_development_manifest()?;
    let reconciliation = crate::deploy::reconcile(repository, &manifest, &head);
    if reconciliation.decision != crate::deploy::DeliveryDecision::NoOp {
        return Err(format!(
            "benchmark requires delivery reconciliation to be no-op, found {}; run scripts/agent deliver first",
            reconciliation.decision.label()
        )
        .into());
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    let output_dir = repository
        .join("build/agent-benchmarks")
        .join(scenario.label())
        .join(timestamp.to_string());

    match scenario {
        BenchmarkScenario::Screensaver => {
            execute_screensaver(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::Particles
        | BenchmarkScenario::ParticleCapacity
        | BenchmarkScenario::ParticleDemo40k
        | BenchmarkScenario::ParticleStep
        | BenchmarkScenario::ParticleProfile => {
            unreachable!("particle scenarios are redirected before installed-runtime preflight")
        }
        BenchmarkScenario::CatalogLifecycle => {
            execute_catalog_lifecycle(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::CatalogBuildRebuild => {
            execute_catalog_build_rebuild(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::CatalogResumeValidation => {
            execute_catalog_resume_validation(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::PreviewWorkAttribution => execute_attribution_capture(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::PreviewWorkAttribution,
            "preview-work-attribution",
            "mister-magik-preview-work-attribution-v1",
        ),
        BenchmarkScenario::CatalogFullBuildRebuild => {
            execute_catalog_full_build_rebuild(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::CatalogCorpusInventory => {
            execute_catalog_corpus_inventory(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::CatalogAttributionControl => execute_catalog_attribution(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::CatalogAttributionControl,
            "control",
        ),
        BenchmarkScenario::CatalogAttributionPprof => execute_catalog_attribution(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::CatalogAttributionPprof,
            "pprof",
        ),
        BenchmarkScenario::CatalogAttributionPmu => execute_catalog_attribution(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::CatalogAttributionPmu,
            "pmu",
        ),
        BenchmarkScenario::CatalogAttributionStorage => execute_catalog_attribution(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::CatalogAttributionStorage,
            "storage",
        ),
        BenchmarkScenario::CatalogAttributionFunctionGraph => execute_catalog_attribution(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::CatalogAttributionFunctionGraph,
            "function-graph",
        ),
        BenchmarkScenario::CatalogAttributionStreamline => execute_catalog_attribution(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::CatalogAttributionStreamline,
            "streamline",
        ),
        BenchmarkScenario::CatalogAttributionReport => execute_catalog_attribution(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::CatalogAttributionReport,
            "report",
        ),
        BenchmarkScenario::SystemEntry => {
            execute_system_entry(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::SystemEntryCritical => {
            execute_system_entry_critical(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::SystemEntryCriticalConfirm => execute_system_entry_repeated(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::SystemEntryCriticalConfirm,
            "mister-magik-system-entry-critical-confirm-benchmark-v2",
            "confirming direct collection entry for the fixed critical systems with 10 fresh processes each",
        ),
        BenchmarkScenario::SystemEntryCriticalProfile => execute_system_entry_repeated(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::SystemEntryCriticalProfile,
            "mister-magik-system-entry-critical-profile-v1",
            "profiling isolated C64 and SNES destination preparation with pprof and per-thread PMU counters",
        ),
        BenchmarkScenario::SystemEntryCriticalStreamline => execute_system_entry_repeated(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::SystemEntryCriticalStreamline,
            "mister-magik-system-entry-critical-streamline-v1",
            "capturing direct C64 and SNES entry inside one bounded system-wide Streamline session",
        ),
        BenchmarkScenario::SystemEntryQualification => execute_system_entry_repeated(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::SystemEntryQualification,
            "mister-magik-system-entry-qualification-benchmark-v2",
            "qualifying direct collection entry for every populated system with 10 fresh processes each",
        ),
        BenchmarkScenario::LaunchReturn => {
            execute_launch_return(&mut device, manifest, output_dir, reporter, false)
        }
        BenchmarkScenario::LaunchReturnOnce => execute_attribution_capture(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::LaunchReturnOnce,
            "launch-return-once",
            "mister-magik-launch-return-once-v2",
        ),
        BenchmarkScenario::LaunchReturnFallback => {
            execute_launch_return(&mut device, manifest, output_dir, reporter, true)
        }
        BenchmarkScenario::LaunchReturnAttribution => execute_attribution_capture(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::LaunchReturnAttribution,
            "launch-return-attribution",
            "mister-magik-launch-return-attribution-v1",
        ),
        BenchmarkScenario::ModalInput => {
            execute_modal_input(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::InputIntegrity => {
            execute_input_integrity(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::LauncherResponse => {
            execute_launcher_response(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::LauncherResponseRetained => execute_attribution_capture(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::LauncherResponseRetained,
            "launcher-response-retained",
            "mister-magik-launcher-response-v2",
        ),
        BenchmarkScenario::LauncherResponseAttribution => {
            execute_launcher_response_attribution(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::GuiFrameAttribution => {
            execute_gui_frame_attribution(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::SettledComposition => execute_attribution_capture(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::SettledComposition,
            "settled-composition",
            "mister-magik-settled-composition-v1",
        ),
        BenchmarkScenario::BridgeModelChurn => execute_attribution_capture(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::BridgeModelChurn,
            "bridge-model-churn",
            "mister-magik-bridge-model-churn-v1",
        ),
        BenchmarkScenario::BridgeModelChurnRetained => execute_attribution_capture(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::BridgeModelChurnRetained,
            "bridge-model-churn-retained",
            "mister-magik-bridge-model-churn-v1",
        ),
        BenchmarkScenario::SchedulerTrace => execute_attribution_capture(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::SchedulerTrace,
            "scheduler-trace",
            "mister-magik-scheduler-trace-v1",
        ),
        BenchmarkScenario::StorageAttribution => execute_attribution_capture(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::StorageAttribution,
            "storage-attribution",
            "mister-magik-storage-attribution-v1",
        ),
        BenchmarkScenario::ArcadeVelocityScroll => {
            execute_arcade_velocity_scroll(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::ArcadeVelocityScrollAttribution => match arm {
            Some(arm) => execute_arcade_velocity_scroll_arm(
                &mut device,
                manifest,
                output_dir,
                reporter,
                ArcadeVelocityScrollRunSpec::new(arm, route)
                    .with_duration_seconds(duration_seconds),
            ),
            None => execute_arcade_velocity_scroll_attribution(
                &mut device,
                manifest,
                output_dir,
                reporter,
            ),
        },
        BenchmarkScenario::TransitionStreamline => execute_attribution_capture(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::TransitionStreamline,
            "transition-streamline",
            "mister-magik-transition-streamline-v1",
        ),
        BenchmarkScenario::AgentObserverAttribution => execute_attribution_capture(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::AgentObserverAttribution,
            "agent-observer-attribution",
            "mister-magik-agent-observer-attribution-v1",
        ),
        BenchmarkScenario::AgentIoAttribution => execute_attribution_capture(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::AgentIoAttribution,
            "agent-io-attribution",
            "mister-magik-agent-io-attribution-v1",
        ),
        BenchmarkScenario::InputLatencyLab => {
            execute_input_latency_lab(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::LauncherResponseStreamline => {
            execute_launcher_response_streamline(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::ColdBoot => execute_cold_boot(
            &mut device,
            manifest,
            output_dir,
            reporter,
            false,
            fresh_catalog,
        ),
        BenchmarkScenario::ColdBootPprof => execute_cold_boot(
            &mut device,
            manifest,
            output_dir,
            reporter,
            true,
            fresh_catalog,
        ),
        BenchmarkScenario::NavigationTransitions => {
            execute_navigation_transitions(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::SettingsNavigation => execute_settings_navigation(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::SettingsNavigation,
            false,
        ),
        BenchmarkScenario::SettingsNavigationPprof => execute_settings_navigation(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::SettingsNavigationPprof,
            true,
        ),
        BenchmarkScenario::OrientationTransitionFade => execute_orientation_transition(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::OrientationTransitionFade,
            "brightness-fade",
            false,
        ),
        BenchmarkScenario::OrientationTransitionZoom => execute_orientation_transition(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::OrientationTransitionZoom,
            "center-pixel-zoom",
            false,
        ),
        BenchmarkScenario::OrientationTransitionFadePprof => execute_orientation_transition(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::OrientationTransitionFadePprof,
            "brightness-fade",
            true,
        ),
        BenchmarkScenario::OrientationTransitionZoomPprof => execute_orientation_transition(
            &mut device,
            manifest,
            output_dir,
            reporter,
            BenchmarkProfile::OrientationTransitionZoomPprof,
            "center-pixel-zoom",
            true,
        ),
        BenchmarkScenario::NeonAttribution => {
            execute_neon_attribution(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::PmuProfile => execute_pmu(&mut device, manifest, output_dir, reporter),
        BenchmarkScenario::MediaPackPersistence => {
            execute_media_pack_persistence(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::RomIdentityHashing => {
            execute_rom_identity_hashing(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::Search => execute_search(&mut device, manifest, output_dir, reporter),
        BenchmarkScenario::Streamline => {
            execute_streamline(&mut device, manifest, output_dir, reporter)
        }
    }
}

fn execute_streamline(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "capturing a bounded Arm Streamline profile",
        Some(30),
    )?;
    let detail = device.profile(BenchmarkProfile::Streamline, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary.get("schema").and_then(Value::as_str) != Some("mister-magik-streamline-capture-v1")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
    {
        return Err("Streamline capture is not a passing v1 report".into());
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_launcher_response_streamline(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "capturing the production launcher-response route in Arm Streamline",
        Some(30),
    )?;
    let detail = device.profile(
        BenchmarkProfile::LauncherResponseStreamline,
        output_dir.clone(),
    )?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-launcher-response-streamline-v1")
        || summary.get("artifact_status").and_then(Value::as_str) != Some("passed")
    {
        return Err("launcher-response Streamline capture is not a valid v1 artifact".into());
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_pmu(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "profiling fixed Cortex-A9 PMU workloads",
        Some(30),
    )?;
    let detail = device.profile(BenchmarkProfile::Pmu, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    evaluate_pmu_summary(&summary)?;
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_neon_attribution(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "profiling fixed Cortex-A9 NEON workloads",
        Some(20),
    )?;
    let detail = device.profile(BenchmarkProfile::NeonAttribution, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary.get("schema").and_then(Value::as_str) != Some("mister-magik-neon-attribution-v1")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
        || summary.get("counter_set").and_then(Value::as_str) != Some("cortex-a9-neon")
    {
        return Err("NEON attribution campaign returned an unusable report".into());
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_system_entry(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "measuring every system from activation to a fully presented game list and screenshot",
        Some(30),
    )?;
    let detail = device.profile(BenchmarkProfile::SystemEntry, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-system-entry-benchmark-v1")
        || summary
            .get("systems")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
    {
        return Err("system-entry benchmark did not produce a passing v1 report".into());
    }
    if summary.get("status").and_then(Value::as_str) != Some("passed") {
        let unready = summary
            .get("unready_systems")
            .and_then(Value::as_array)
            .map(|systems| {
                systems
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        return Err(format!(
            "system-entry benchmark retained a failed summary at {}; unready systems: {}",
            output_dir.join("summary.json").display(),
            if unready.is_empty() {
                "unknown"
            } else {
                &unready
            }
        )
        .into());
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_system_entry_critical(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "measuring direct collection entry for the fixed critical systems",
        Some(30),
    )?;
    let detail = device.profile(BenchmarkProfile::SystemEntryCritical, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-system-entry-critical-benchmark-v3")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
        || summary
            .get("systems")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
    {
        return Err(format!(
            "system-entry-critical retained a failed summary at {}",
            output_dir.join("summary.json").display()
        )
        .into());
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_system_entry_repeated(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
    profile: BenchmarkProfile,
    expected_schema: &str,
    progress: &str,
) -> AgentResult<Outcome> {
    reporter.emit(EventKind::Progress, "profile", progress, Some(30))?;
    let detail = device.profile(profile, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary.get("schema").and_then(Value::as_str) != Some(expected_schema)
        || summary.get("status").and_then(Value::as_str) != Some("passed")
        || summary
            .get("systems")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
    {
        return Err(format!(
            "{} retained a failed summary at {}",
            profile.label(),
            output_dir.join("summary.json").display()
        )
        .into());
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

impl BenchmarkProfile {
    const fn label(self) -> &'static str {
        match self {
            Self::SystemEntryCriticalConfirm => "system-entry-critical-confirm",
            Self::SystemEntryCriticalProfile => "system-entry-critical-profile",
            Self::SystemEntryCriticalStreamline => "system-entry-critical-streamline",
            Self::SystemEntryQualification => "system-entry-qualification",
            _ => "system-entry benchmark",
        }
    }
}

fn evaluate_pmu_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str) != Some("mister-magik-pmu-suite-v2")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
    {
        return Err("PMU suite summary is not a passing v2 report".into());
    }
    let workloads = summary
        .get("workloads")
        .and_then(Value::as_array)
        .ok_or("PMU suite summary has no workloads")?;
    if workloads.len() != 4 {
        return Err(format!(
            "PMU suite expected four workloads, received {}",
            workloads.len()
        )
        .into());
    }
    for workload in workloads {
        if workload.get("status").and_then(Value::as_str) != Some("ok") {
            return Err("PMU suite contains a failed workload".into());
        }
    }
    Ok(())
}

fn particle_scene_lab_command(scenario: BenchmarkScenario) -> Option<&'static str> {
    match scenario {
        BenchmarkScenario::Particles => Some(
            "particle qualification moved to the dedicated lab; sweep --particle-count with: scripts/agent device scene-lab --scene magik --recipe crates/particles/assets/recipes/magik-v1.json --particle-preset visual --particle-count COUNT --seconds 90 --assess --attended",
        ),
        BenchmarkScenario::ParticleCapacity => Some(
            "particle capacity qualification moved to the dedicated lab; sweep --particle-count with: scripts/agent device scene-lab --scene magik --recipe crates/particles/assets/recipes/magik-v1.json --particle-preset capacity --particle-count COUNT --seconds 90 --assess --attended",
        ),
        BenchmarkScenario::ParticleDemo40k => Some(
            "run: scripts/agent device scene-lab --scene magik --recipe crates/particles/assets/recipes/magik-v1.json --particle-preset visual --particle-count 40960 --seconds 90 --assess --attended",
        ),
        BenchmarkScenario::ParticleStep => Some(
            "run: scripts/agent device scene-lab --scene magik --recipe crates/particles/assets/recipes/magik-v1.json --particle-preset capacity --particle-count 14336 --seconds 90 --assess --attended",
        ),
        BenchmarkScenario::ParticleProfile => Some(
            "particle CPU profiling moved to the dedicated lab; run the required count with scripts/agent device scene-lab --scene magik --recipe crates/particles/assets/recipes/magik-v1.json --particle-preset PRESET --particle-count COUNT --seconds 90 --assess --attended",
        ),
        BenchmarkScenario::Streamline
        | BenchmarkScenario::Screensaver
        | BenchmarkScenario::ColdBoot
        | BenchmarkScenario::ColdBootPprof
        | BenchmarkScenario::CatalogLifecycle
        | BenchmarkScenario::CatalogBuildRebuild
        | BenchmarkScenario::CatalogResumeValidation
        | BenchmarkScenario::CatalogFullBuildRebuild
        | BenchmarkScenario::CatalogCorpusInventory
        | BenchmarkScenario::CatalogAttributionControl
        | BenchmarkScenario::CatalogAttributionPprof
        | BenchmarkScenario::CatalogAttributionPmu
        | BenchmarkScenario::CatalogAttributionStorage
        | BenchmarkScenario::CatalogAttributionFunctionGraph
        | BenchmarkScenario::CatalogAttributionStreamline
        | BenchmarkScenario::CatalogAttributionReport
        | BenchmarkScenario::SystemEntry
        | BenchmarkScenario::SystemEntryCritical
        | BenchmarkScenario::SystemEntryCriticalConfirm
        | BenchmarkScenario::SystemEntryCriticalProfile
        | BenchmarkScenario::SystemEntryCriticalStreamline
        | BenchmarkScenario::SystemEntryQualification
        | BenchmarkScenario::LaunchReturn
        | BenchmarkScenario::LaunchReturnOnce
        | BenchmarkScenario::LaunchReturnFallback
        | BenchmarkScenario::LaunchReturnAttribution
        | BenchmarkScenario::ModalInput
        | BenchmarkScenario::InputIntegrity
        | BenchmarkScenario::LauncherResponse
        | BenchmarkScenario::LauncherResponseRetained
        | BenchmarkScenario::LauncherResponseAttribution
        | BenchmarkScenario::GuiFrameAttribution
        | BenchmarkScenario::SettledComposition
        | BenchmarkScenario::BridgeModelChurn
        | BenchmarkScenario::BridgeModelChurnRetained
        | BenchmarkScenario::SchedulerTrace
        | BenchmarkScenario::StorageAttribution
        | BenchmarkScenario::ArcadeVelocityScroll
        | BenchmarkScenario::ArcadeVelocityScrollAttribution
        | BenchmarkScenario::TransitionStreamline
        | BenchmarkScenario::AgentObserverAttribution
        | BenchmarkScenario::AgentIoAttribution
        | BenchmarkScenario::InputLatencyLab
        | BenchmarkScenario::LauncherResponseStreamline
        | BenchmarkScenario::NavigationTransitions
        | BenchmarkScenario::SettingsNavigation
        | BenchmarkScenario::SettingsNavigationPprof
        | BenchmarkScenario::OrientationTransitionFade
        | BenchmarkScenario::OrientationTransitionZoom
        | BenchmarkScenario::OrientationTransitionFadePprof
        | BenchmarkScenario::OrientationTransitionZoomPprof
        | BenchmarkScenario::NeonAttribution
        | BenchmarkScenario::PmuProfile
        | BenchmarkScenario::MediaPackPersistence
        | BenchmarkScenario::RomIdentityHashing
        | BenchmarkScenario::PreviewWorkAttribution
        | BenchmarkScenario::Search => None,
    }
}

fn execute_orientation_transition(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
    profile: BenchmarkProfile,
    effect: &str,
    profiler_enabled: bool,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        if profiler_enabled {
            "profile"
        } else {
            "qualification"
        },
        &format!(
            "{} six real Settings {effect} orientation transitions",
            if profiler_enabled {
                "profiling"
            } else {
                "qualifying"
            }
        ),
        Some(20),
    )?;
    let detail = device.profile(profile, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-orientation-transition-qualification-v1")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
        || summary.get("effect").and_then(Value::as_str) != Some(effect)
        || summary.get("profiler_enabled").and_then(Value::as_bool) != Some(profiler_enabled)
    {
        return Err("orientation transition run did not complete its isolated effect".into());
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_settings_navigation(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
    profile: BenchmarkProfile,
    profiler_enabled: bool,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        if profiler_enabled {
            "profile"
        } else {
            "qualification"
        },
        &format!(
            "{} six landscape then six portrait-left Settings navigation transitions",
            if profiler_enabled {
                "profiling"
            } else {
                "qualifying"
            }
        ),
        Some(20),
    )?;
    let detail = device.profile(profile, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-settings-navigation-qualification-v4")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
        || summary
            .get("orientations")
            .and_then(Value::as_array)
            .map(Vec::len)
            != Some(2)
        || summary.get("profiler_enabled").and_then(Value::as_bool) != Some(profiler_enabled)
    {
        return Err("Settings navigation run did not complete both orientations".into());
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_modal_input(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "ui-verification",
        "verifying exclusive modal input on the installed Dev runtime",
        Some(35),
    )?;
    let detail = device.profile(BenchmarkProfile::ModalInput, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    evaluate_modal_input_summary(&summary)?;
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_input_integrity(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "input-integrity",
        "driving bounded pulses through Main proxy v2 and the kernel input path",
        Some(35),
    )?;
    let detail = device.profile(BenchmarkProfile::InputIntegrity, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    evaluate_input_integrity_summary(&summary)?;
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_launcher_response(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "launcher-response",
        "verifying latch-confirmed launcher response through Main proxy v2",
        Some(35),
    )?;
    let detail = device.profile(BenchmarkProfile::LauncherResponse, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    evaluate_launcher_response_summary(&summary)?;
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_launcher_response_attribution(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "launcher-response-attribution",
        "collecting control, execution, pprof, PMU, and Streamline evidence",
        Some(35),
    )?;
    let detail = device.profile(
        BenchmarkProfile::LauncherResponseAttribution,
        output_dir.clone(),
    )?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary["schema"].as_str() != Some("mister-magik-launcher-response-attribution-v1")
        || summary["artifact_status"].as_str() != Some("passed")
        || summary["arms"].as_array().map(Vec::len) != Some(5)
    {
        return Err("launcher-response attribution did not produce complete v1 evidence".into());
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_gui_frame_attribution(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "gui-frame-attribution",
        "collecting independent control, PMU, and system-wide Streamline GUI frame evidence",
        Some(35),
    )?;
    let detail = device.profile(BenchmarkProfile::GuiFrameAttribution, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-gui-frame-attribution-v1")
        || summary.get("artifact_status").and_then(Value::as_str) != Some("passed")
        || !matches!(summary.pointer("/arms/control"), Some(Value::Object(_)))
        || !matches!(summary.pointer("/arms/pmu"), Some(Value::Object(_)))
        || !matches!(summary.pointer("/arms/streamline"), Some(Value::Object(_)))
    {
        return Err("GUI frame attribution did not produce complete v1 evidence".into());
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_arcade_velocity_scroll(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "arcade-velocity-scroll",
        "measuring one fixed 40-second Arcade velocity scroll on the active display mode",
        Some(35),
    )?;
    let detail = device.profile(BenchmarkProfile::ArcadeVelocityScroll, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-arcade-velocity-scroll-v1")
        || !matches!(summary.get("quality_status"), Some(Value::String(_)))
    {
        return Err("Arcade velocity-scroll benchmark did not produce complete v1 evidence".into());
    }
    let quality_passed = summary.get("quality_status").and_then(Value::as_str) == Some("passed");
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    if !quality_passed {
        return Err(format!(
            "Arcade velocity-scroll cadence failed; evidence retained at {}",
            output_dir.display()
        )
        .into());
    }
    Ok(Outcome::Passed)
}

fn execute_arcade_velocity_scroll_attribution(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "arcade-velocity-scroll-attribution",
        "collecting control, pprof, PMU, and Streamline evidence without changing the active route",
        Some(25),
    )?;
    let detail = device.profile(
        BenchmarkProfile::ArcadeVelocityScrollAttribution,
        output_dir.clone(),
    )?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-arcade-velocity-scroll-attribution-v1")
        || summary.get("artifact_status").and_then(Value::as_str) != Some("passed")
        || !matches!(summary.get("arms"), Some(Value::Object(arms)) if arms.len() == 4)
    {
        return Err(
            "Arcade velocity-scroll attribution did not produce complete v1 evidence".into(),
        );
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
            "performance_authority": "unprofiled control arm only",
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_arcade_velocity_scroll_arm(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
    spec: ArcadeVelocityScrollRunSpec,
) -> AgentResult<Outcome> {
    let arm = spec.arm;
    let schema = match arm {
        ArcadeVelocityScrollArm::Control | ArcadeVelocityScrollArm::Turbo => {
            "mister-magik-arcade-velocity-scroll-v1"
        }
        ArcadeVelocityScrollArm::Pprof => "mister-magik-arcade-velocity-scroll-pprof-v1",
        ArcadeVelocityScrollArm::Pmu => "mister-magik-arcade-velocity-scroll-pmu-v1",
        ArcadeVelocityScrollArm::Streamline => "mister-magik-arcade-velocity-scroll-streamline-v1",
    };
    reporter.emit(
        EventKind::Progress,
        "arcade-velocity-scroll-attribution",
        &format!(
            "collecting only the {} arm for {} seconds on the {} route",
            arm.label(),
            spec.duration_ms / 1_000,
            spec.route.label(),
        ),
        Some(25),
    )?;
    let detail = device.profile_arcade_velocity_scroll(spec, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    let schema_valid = summary.get("schema").and_then(Value::as_str) == Some(schema);
    let artifact_valid = match arm {
        ArcadeVelocityScrollArm::Control | ArcadeVelocityScrollArm::Turbo => {
            matches!(summary.get("quality_status"), Some(Value::String(_)))
        }
        _ => summary.get("artifact_status").and_then(Value::as_str) == Some("passed"),
    };
    if !schema_valid || !artifact_valid {
        return Err(format!(
            "Arcade velocity-scroll {} arm did not produce complete {schema} evidence",
            arm.label()
        )
        .into());
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "selected_arm": arm.label(),
            "requested_route": spec.route.label(),
            "summary": summary,
            "output_dir": output_dir,
            "performance_authority": if matches!(
                arm,
                ArcadeVelocityScrollArm::Control | ArcadeVelocityScrollArm::Turbo
            ) && spec.duration_ms >= 40_000 {
                "unprofiled control"
            } else if matches!(
                arm,
                ArcadeVelocityScrollArm::Control | ArcadeVelocityScrollArm::Turbo
            ) {
                "directional development evidence"
            } else {
                "diagnostic attribution only"
            },
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_attribution_capture(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
    profile: BenchmarkProfile,
    label: &str,
    schema: &str,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        label,
        &format!("collecting the fixed {label} attribution route"),
        Some(35),
    )?;
    let detail = device.profile(profile, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    if summary.get("schema").and_then(Value::as_str) != Some(schema)
        || summary.get("artifact_status").and_then(Value::as_str) != Some("passed")
    {
        return Err(format!("{label} did not produce complete identity-bound evidence").into());
    }
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_input_latency_lab(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "input-latency-lab",
        "running the fixed on-device input latency experiment",
        Some(35),
    )?;
    let detail = device.profile(BenchmarkProfile::InputLatencyLab, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    evaluate_input_latency_lab_summary(&summary)?;
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn evaluate_input_latency_lab_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str) != Some("mister-magik-input-latency-lab-v1")
        || summary.get("status").and_then(Value::as_str) != Some("completed")
        || summary
            .get("artifact_validity_status")
            .and_then(Value::as_str)
            != Some("passed")
        || summary.get("arms").and_then(Value::as_array).map(Vec::len) != Some(9)
    {
        return Err("input latency laboratory did not produce complete v1 evidence".into());
    }
    Ok(())
}

fn evaluate_launcher_response_summary(summary: &Value) -> AgentResult<()> {
    let diagnostic_mode = summary.get("diagnostic_mode").and_then(Value::as_str);
    let background_adoption_applicable = summary
        .get("background_adoption_applicable")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if summary.get("schema").and_then(Value::as_str) != Some("mister-magik-launcher-response-v2")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
        || !matches!(summary.get("protocol").and_then(Value::as_u64), Some(2 | 3))
        || summary.get("input_response_status").and_then(Value::as_str) != Some("passed")
        || summary.get("pulse_status").and_then(Value::as_str) != Some("passed")
        || summary.get("integrity_status").and_then(Value::as_str) != Some("passed")
        || (background_adoption_applicable
            && summary
                .get("background_adoption_status")
                .and_then(Value::as_str)
                != Some("passed"))
        || (!background_adoption_applicable
            && summary
                .get("background_adoption_status")
                .and_then(Value::as_str)
                != Some("not-applicable"))
        || summary
            .get("dispatch_p95_us")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
            > 3_000
        || summary
            .get("dispatch_max_us")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
            > 5_000
        || summary
            .get("confirmed_median_us")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
            > 12_000
        || summary.get("lost_actions").and_then(Value::as_u64) != Some(0)
        || summary.get("duplicated_actions").and_then(Value::as_u64) != Some(0)
        || summary.get("coalesced_actions").and_then(Value::as_u64) != Some(0)
        || summary.get("reordered_actions").and_then(Value::as_u64) != Some(0)
        || summary.get("proxy_write_failures").and_then(Value::as_u64) != Some(0)
        || summary.get("journal_overflows").and_then(Value::as_u64) != Some(0)
        || summary.get("sequence_gaps").and_then(Value::as_u64) != Some(0)
        || summary.get("latch_drops").and_then(Value::as_u64) != Some(0)
        || (diagnostic_mode.is_none()
            && summary.get("repeated_vblanks").and_then(Value::as_u64) != Some(0))
        || summary.get("ownership_losses").and_then(Value::as_u64) != Some(0)
        || (background_adoption_applicable
            && (summary
                .get("catalog_adoption_max_us")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX)
                >= 8_000
                || summary
                    .get("catalog_adoption_max_us")
                    .and_then(Value::as_u64)
                    == Some(0)))
    {
        return Err(
            "launcher response qualification did not satisfy the game-quality gates".into(),
        );
    }
    Ok(())
}

fn evaluate_input_integrity_summary(summary: &Value) -> AgentResult<()> {
    let protocol = summary.get("protocol").and_then(Value::as_u64);
    if summary.get("schema").and_then(Value::as_str) != Some("mister-magik-input-integrity-v2")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
        || !matches!(protocol, Some(2 | 3))
        || summary.get("lost_actions").and_then(Value::as_u64) != Some(0)
        || summary.get("duplicated_actions").and_then(Value::as_u64) != Some(0)
        || summary.get("proxy_write_failures").and_then(Value::as_u64) != Some(0)
        || summary.get("journal_overflows").and_then(Value::as_u64) != Some(0)
        || summary.get("sequence_gaps").and_then(Value::as_u64) != Some(0)
    {
        return Err("input integrity qualification did not satisfy the zero-loss gates".into());
    }
    Ok(())
}

fn evaluate_modal_input_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-modal-input-verification-v1")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
        || summary
            .pointer("/during_hold/return_screen")
            .and_then(Value::as_str)
            != Some("home")
        || summary
            .pointer("/after_release/return_screen")
            .and_then(Value::as_str)
            != Some("home")
        || summary
            .pointer("/fresh_press/return_screen")
            .and_then(Value::as_str)
            != Some("arcade")
        || summary
            .pointer("/rebuild_selected/dialog_selected")
            .and_then(Value::as_i64)
            != Some(1)
    {
        return Err("modal input verification did not preserve exclusive dialog input".into());
    }
    Ok(())
}

fn require_active_development_runtime(active: &crate::host::ActiveRuntime) -> AgentResult<()> {
    if active.is_development_launcher() {
        Ok(())
    } else {
        Err(format!(
            "benchmark requires the active development launcher, found {}; run scripts/agent deliver",
            active.description()
        )
        .into())
    }
}

fn execute_cold_boot(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
    pprof: bool,
    fresh_catalog: bool,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        if pprof {
            "sampling one controlled Dev cold boot from MagiK process entry through the first real launcher frame"
        } else {
            "profiling one controlled Dev cold boot through the first real launcher frame"
        },
        Some(35),
    )?;
    let profile = BenchmarkProfile::ColdBoot {
        pprof,
        fresh_catalog,
    };
    let detail = device.profile(profile, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    evaluate_cold_boot_summary(&summary, pprof, fresh_catalog)?;
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn evaluate_cold_boot_summary(
    summary: &Value,
    pprof: bool,
    fresh_catalog: bool,
) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str) != Some("mister-magik-cold-boot-benchmark-v1")
        || summary.get("timing_class").and_then(Value::as_str)
            != Some("device-monotonic-instrumented-installed-dev")
    {
        return Err("cold-boot benchmark summary has the wrong evidence schema".into());
    }
    let timeline = summary
        .get("timeline")
        .and_then(Value::as_object)
        .ok_or("cold-boot benchmark summary has no timeline")?;
    let ordered = [
        "agent_start_us",
        "initial_main_entry_us",
        "final_main_entry_us",
        "preflight_begin_us",
        "preflight_end_us",
        "launcher_exec_us",
        "magik_process_start_us",
        "first_launcher_present_us",
    ]
    .into_iter()
    .map(|field| timeline.get(field).and_then(Value::as_u64).unwrap_or(0))
    .collect::<Vec<_>>();
    if ordered[0] == 0 || ordered.windows(2).any(|pair| pair[0] > pair[1]) {
        return Err(
            format!("cold-boot benchmark timestamps are zero or unordered: {ordered:?}").into(),
        );
    }
    if summary.get("capture_verified").and_then(Value::as_bool) != Some(true)
        || summary.get("launcher_ready").and_then(Value::as_bool) != Some(true)
    {
        return Err("cold-boot benchmark did not verify the visible launcher".into());
    }
    if summary.get("fresh_catalog").and_then(Value::as_bool) != Some(fresh_catalog) {
        return Err("cold-boot benchmark reported the wrong catalog mode".into());
    }
    if fresh_catalog
        && !summary
            .pointer("/timeline/magik_startup_events")
            .and_then(Value::as_array)
            .is_some_and(|events| {
                events.iter().any(|event| {
                    event.get("event").and_then(Value::as_str) == Some("startup_entry_classified")
                        && event
                            .get("detail")
                            .and_then(Value::as_str)
                            .is_some_and(|detail| detail.contains("mode=cold_no_catalog"))
                })
            })
    {
        return Err("cold-boot benchmark did not enter the fresh-catalog startup path".into());
    }
    if pprof
        && (summary.pointer("/profile/state").and_then(Value::as_str) != Some("complete")
            || summary
                .pointer("/profile/sample_hits")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                <= 0)
    {
        return Err("cold-boot pprof benchmark produced no CPU samples".into());
    }
    Ok(())
}

const fn benchmark_requires_initial_health(scenario: BenchmarkScenario) -> bool {
    // Launch-return owns a one-shot launcher.env. Its typed operation removes that
    // exact state and then performs the same development health check itself.
    !matches!(
        scenario,
        BenchmarkScenario::LaunchReturn
            | BenchmarkScenario::LaunchReturnOnce
            | BenchmarkScenario::LaunchReturnFallback
            | BenchmarkScenario::LaunchReturnAttribution
    )
}

fn execute_launch_return(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
    force_capsule_miss: bool,
) -> AgentResult<Outcome> {
    let scenario = if force_capsule_miss {
        "launch-return-fallback"
    } else {
        "launch-return"
    };
    reporter.emit(
        EventKind::Progress,
        "profile",
        if force_capsule_miss {
            "profiling one normal Arcade return and one forced capsule-miss return on the coherently installed Dev runtime"
        } else {
            "profiling two Arcade launch-return cycles on the coherently installed Dev runtime"
        },
        Some(35),
    )?;
    let profile = if force_capsule_miss {
        BenchmarkProfile::LaunchReturnFallback
    } else {
        BenchmarkProfile::LaunchReturn
    };
    let detail = device.profile(profile, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    evaluate_launch_return_summary(&summary, scenario, force_capsule_miss)?;
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn evaluate_launch_return_summary(
    summary: &Value,
    expected_scenario: &str,
    expect_capsule_fallback: bool,
) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-launch-return-benchmark-v3")
    {
        return Err("launch-return benchmark summary has the wrong schema".into());
    }
    if summary.get("scenario").and_then(Value::as_str) != Some(expected_scenario) {
        return Err("launch-return benchmark summary has the wrong scenario".into());
    }
    if summary.get("timing_class").and_then(Value::as_str)
        != Some("instrumented-installed-dev-symbols")
        || summary.get("latency").and_then(Value::as_object).is_none()
    {
        return Err(
            "launch-return benchmark summary lacks installed-runtime profile evidence".into(),
        );
    }
    for (field, length) in [
        ("main_revision", 40),
        ("main_sha256", 64),
        ("magik_revision", 40),
        ("gui_sha256", 64),
    ] {
        let value = summary.get(field).and_then(Value::as_str).unwrap_or("");
        if value.len() != length
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!("launch-return benchmark has invalid {field}").into());
        }
    }
    for phase in [
        "command_to_process",
        "process_to_context",
        "context_to_preview",
        "preview_to_present",
        "total_return",
    ] {
        if summary
            .pointer(&format!("/latency/{phase}/median_us"))
            .and_then(Value::as_u64)
            .is_none()
        {
            return Err(format!("launch-return benchmark has no {phase} latency").into());
        }
    }
    let cycles = summary
        .get("cycles")
        .and_then(Value::as_array)
        .ok_or("launch-return benchmark summary has no cycles")?;
    if cycles.len() != 2 {
        return Err(format!(
            "launch-return benchmark completed {} of 2 cycles",
            cycles.len()
        )
        .into());
    }
    for (index, cycle) in cycles.iter().enumerate() {
        let capsule_fault_injected = cycle
            .get("capsule_fault_injected")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if capsule_fault_injected != (expect_capsule_fallback && index == 1) {
            return Err(format!(
                "launch-return cycle {} has the wrong capsule fault state",
                index + 1
            )
            .into());
        }
        if capsule_fault_injected
            && cycle.get("return_source").and_then(Value::as_str) != Some("sharded-registry")
        {
            return Err("forced capsule miss did not restore from the sharded registry".into());
        }
        let restored = cycle.get("restored").and_then(Value::as_bool) == Some(true);
        let elapsed_ms = cycle
            .get("black_interval_ms")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX);
        let total_return_us = cycle
            .get("total_return_us")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let timestamps = [
            "request_monotonic_us",
            "acknowledged_monotonic_us",
            "process_start_monotonic_us",
            "exact_context_monotonic_us",
            "preview_ready_monotonic_us",
            "first_correct_present_monotonic_us",
        ]
        .map(|field| cycle.get(field).and_then(Value::as_u64).unwrap_or(0));
        let monotonic_ordered = timestamps[0] > 0
            && timestamps.windows(2).all(|pair| pair[0] <= pair[1])
            && total_return_us == timestamps[5] - timestamps[0];
        let black_interval_authoritative =
            elapsed_ms == total_return_us.saturating_add(999) / 1_000;
        let preview_verified = cycle.get("preview_verified").and_then(Value::as_bool) == Some(true);
        let cpu_profile_valid = cycle
            .get("cpu_sample_hits")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            > 0
            && cycle
                .get("cpu_sample_stacks")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0
            && cycle
                .get("resolved_application_symbols")
                .and_then(Value::as_bool)
                == Some(true);
        let artifacts_present = [
            "capture_file",
            "capture_metadata_file",
            "flamegraph_file",
            "folded_stacks_file",
            "cpu_profile_file",
            "frame_profile_file",
            "timeline_file",
        ]
        .iter()
        .all(|field| {
            cycle
                .get(*field)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        });
        if !restored
            || elapsed_ms >= 5_000
            || total_return_us == 0
            || !monotonic_ordered
            || !black_interval_authoritative
            || !preview_verified
            || !cpu_profile_valid
            || !artifacts_present
        {
            return Err(format!(
                "launch-return cycle {} failed: restored={restored} black_interval_ms={elapsed_ms} total_return_us={total_return_us} monotonic_ordered={monotonic_ordered} black_interval_authoritative={black_interval_authoritative} preview_verified={preview_verified} artifacts_present={artifacts_present}",
                index + 1
            )
            .into());
        }
    }
    Ok(())
}

fn execute_navigation_transitions(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "profiling scripted launcher navigation transitions",
        Some(20),
    )?;
    let detail = device.profile(BenchmarkProfile::NavigationTransitions, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    device.verify_health()?;
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-navigation-transition-profile-v2")
    {
        return Err("navigation transition profile summary has the wrong schema".into());
    }
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_screensaver(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "profiling installed screensaver",
        Some(20),
    )?;
    let detail = device.profile(BenchmarkProfile::Screensaver, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    device.verify_health()?;
    evaluate_summary(&summary)?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_search(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "profiling installed persisted search",
        Some(30),
    )?;
    let timing_detail = device.profile(BenchmarkProfile::Search, output_dir.join("timing"))?;
    let timing: Value = serde_json::from_str(&timing_detail).map_err(|error| error.to_string())?;
    evaluate_search_summary(&timing)?;
    reporter.emit(
        EventKind::Progress,
        "ui-verification",
        "verifying persisted search through the launcher UI",
        Some(60),
    )?;
    let ui_detail = device.profile(BenchmarkProfile::SearchUi, output_dir.join("ui"))?;
    let ui: Value = serde_json::from_str(&ui_detail).map_err(|error| error.to_string())?;
    evaluate_search_ui_summary(&ui)?;
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "timing": timing,
            "ui": ui,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_rom_identity_hashing(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "measuring production ROM identity hashing",
        Some(30),
    )?;
    let detail = device.profile(BenchmarkProfile::RomIdentityHashing, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    evaluate_rom_identity_hashing_summary(&summary)?;
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_media_pack_persistence(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "measuring raw media-pack persistence",
        Some(30),
    )?;
    let detail = device.profile(BenchmarkProfile::MediaPackPersistence, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    evaluate_media_pack_persistence_summary(&summary)?;
    device.verify_health()?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn evaluate_media_pack_persistence_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-media-pack-persistence-v1")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
        || summary.get("save_strategy").and_then(Value::as_str) != Some("stream-fat")
        || summary.get("decode_ms").and_then(Value::as_u64) != Some(0)
        || summary.get("row_count").and_then(Value::as_u64) != Some(9)
    {
        return Err("media-pack persistence benchmark is not a passing production report".into());
    }
    Ok(())
}

fn evaluate_rom_identity_hashing_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-rom-identity-production-v1")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
    {
        return Err("ROM identity benchmark is not a passing production report".into());
    }
    let runs = summary
        .get("runs")
        .and_then(Value::as_array)
        .ok_or("ROM identity production report has no runs")?;
    if runs.len() != 3
        || runs.iter().any(|run| {
            run.get("case_count").and_then(Value::as_u64).unwrap_or(0) == 0
                || run
                    .get("production_default_selected")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    == 0
                || run.get("implementation").and_then(Value::as_str)
                    != Some("streaming-slicing-by-eight-crc32")
        })
    {
        return Err("ROM identity production report has invalid runs".into());
    }
    Ok(())
}

fn evaluate_search_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str) != Some("mister-magik-search-benchmark-v2") {
        return Err("persisted search benchmark summary has the wrong schema".into());
    }
    let runs = summary
        .get("runs")
        .and_then(Value::as_array)
        .ok_or("persisted search benchmark summary has no runs")?;
    if runs.len() != 3
        || runs.iter().any(|run| {
            run.get("queries")
                .and_then(Value::as_array)
                .is_none_or(|queries| {
                    queries.len() != 4
                        || queries.iter().any(|query| {
                            query
                                .get("result_hash")
                                .and_then(Value::as_str)
                                .is_none_or(|hash| hash.len() != 64)
                        })
                })
        })
    {
        return Err("persisted search benchmark lacks three exact query suites".into());
    }
    if summary
        .pointer("/warm_all_queries/total_us/p95")
        .and_then(Value::as_u64)
        .is_none()
    {
        return Err("persisted search benchmark has no warm total p95 timing".into());
    }
    Ok(())
}

fn evaluate_search_ui_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-search-ui-verification-v1")
        || summary.get("status").and_then(Value::as_str) != Some("ready")
        || summary.get("query").and_then(Value::as_str) != Some("A")
        || summary.get("results").and_then(Value::as_u64).unwrap_or(0) == 0
    {
        return Err("persisted search UI verification did not reach ready results".into());
    }
    Ok(())
}

fn execute_catalog_lifecycle(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "profiling isolated catalog lifecycle",
        Some(35),
    )?;
    let detail = device.profile(BenchmarkProfile::CatalogLifecycle, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    device.verify_health()?;
    evaluate_catalog_lifecycle_summary(&summary)?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_catalog_build_rebuild(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "benchmarking bounded real Arcade, SNES, and C64 catalog build/rebuild",
        Some(35),
    )?;
    let detail = device.profile(BenchmarkProfile::CatalogBuildRebuild, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    device.verify_health()?;
    evaluate_catalog_build_rebuild_summary(&summary)?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_catalog_resume_validation(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "benchmarking handled interrupted catalog resume validation",
        Some(35),
    )?;
    let detail = device.profile(
        BenchmarkProfile::CatalogResumeValidation,
        output_dir.clone(),
    )?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    device.verify_health()?;
    evaluate_catalog_resume_validation_summary(&summary)?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn evaluate_catalog_resume_validation_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-catalog-resume-validation-v3")
        || summary.get("scenario").and_then(Value::as_str) != Some("catalog-resume-validation")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
    {
        return Err("catalog resume-validation summary is not a passing v3 report".into());
    }
    let samples = summary
        .get("samples")
        .and_then(Value::as_array)
        .ok_or("catalog resume-validation summary has no samples")?;
    if samples.len() != 3
        || samples.iter().any(|sample| {
            sample
                .pointer("/resume_metrics/resume_reused")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                == 0
                || sample
                    .pointer("/artifact_set_valid")
                    .and_then(Value::as_bool)
                    != Some(true)
        })
        || summary
            .pointer("/production_registry/unchanged")
            .and_then(Value::as_bool)
            != Some(true)
        || samples.iter().any(|sample| {
            sample.get("arm").and_then(Value::as_str) != Some("production")
                || sample
                    .pointer("/resume_metrics/resume_validation_backend")
                    .and_then(Value::as_str)
                    != Some("walker-native")
        })
    {
        return Err(
            "catalog resume-validation lacks three production reusable exact samples".into(),
        );
    }
    Ok(())
}

fn evaluate_catalog_build_rebuild_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-catalog-build-rebuild-v3")
        || summary.get("scenario").and_then(Value::as_str) != Some("catalog-build-rebuild")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
    {
        return Err("catalog build/rebuild benchmark summary is not a passing v3 report".into());
    }
    let samples = summary
        .get("samples")
        .and_then(Value::as_array)
        .ok_or("catalog build/rebuild benchmark summary has no samples")?;
    if samples.len() != 3 {
        return Err(format!(
            "catalog build/rebuild benchmark expected three samples, received {}",
            samples.len()
        )
        .into());
    }
    if summary
        .pointer("/configuration/unmeasured_warmups")
        .and_then(Value::as_u64)
        != Some(1)
        || summary
            .pointer("/production_registry/unchanged")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err(
            "catalog build/rebuild benchmark lacks warm-up or registry preservation evidence"
                .into(),
        );
    }
    if samples.iter().any(|sample| {
        sample.get("status").and_then(Value::as_str) != Some("passed")
            || sample
                .pointer("/fresh/catalog/valid")
                .and_then(Value::as_bool)
                != Some(true)
            || sample
                .pointer("/rebuild/catalog/valid")
                .and_then(Value::as_bool)
                != Some(true)
            || !catalog_identity_complete(&sample["fresh"]["catalog"])
            || !catalog_identity_complete(&sample["rebuild"]["catalog"])
            || sample
                .pointer("/validation/snes_game_delta")
                .and_then(Value::as_i64)
                != Some(1)
    }) {
        return Err("catalog build/rebuild benchmark contains a failed sample".into());
    }
    Ok(())
}

fn execute_catalog_full_build_rebuild(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "benchmarking one isolated whole-card fresh build and forced rebuild",
        Some(35),
    )?;
    let detail = device.profile(
        BenchmarkProfile::CatalogFullBuildRebuild,
        output_dir.clone(),
    )?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    device.verify_health()?;
    evaluate_catalog_full_build_rebuild_summary(&summary)?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_catalog_corpus_inventory(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "inventorying production-planned catalog targets without publication",
        Some(35),
    )?;
    let detail = device.profile(BenchmarkProfile::CatalogCorpusInventory, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    device.verify_health()?;
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-catalog-corpus-inventory-summary-v1")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
    {
        return Err("catalog corpus inventory is not a passing v1 report".into());
    }
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn evaluate_catalog_full_build_rebuild_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-catalog-full-build-rebuild-v3")
        || summary.get("scenario").and_then(Value::as_str) != Some("catalog-full-build-rebuild")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
    {
        return Err("whole-card catalog benchmark summary is not a passing v3 report".into());
    }
    if summary
        .pointer("/first_observed_clean/catalog/valid")
        .and_then(Value::as_bool)
        != Some(true)
        || !catalog_identity_complete(&summary["first_observed_clean"]["catalog"])
        || summary
            .pointer("/warm_clean/catalog/valid")
            .and_then(Value::as_bool)
            != Some(true)
        || !catalog_identity_complete(&summary["warm_clean"]["catalog"])
        || summary
            .pointer("/rebuild/catalog/valid")
            .and_then(Value::as_bool)
            != Some(true)
        || !catalog_identity_complete(&summary["rebuild"]["catalog"])
        || summary
            .pointer("/validation/exact_identities_identical")
            .and_then(Value::as_bool)
            != Some(true)
        || summary
            .pointer("/validation/artifact_sets_valid")
            .and_then(Value::as_bool)
            != Some(true)
        || summary
            .pointer("/validation/phase_evidence_complete")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err("whole-card catalog benchmark failed catalog validation".into());
    }
    Ok(())
}

fn catalog_identity_complete(catalog: &Value) -> bool {
    [
        "identity_sha256",
        "ordering_sha256",
        "launch_sha256",
        "search_sha256",
        "artifact_set_sha256",
    ]
    .into_iter()
    .all(|field| {
        catalog
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|digest| digest.len() == 64)
    }) && catalog
        .get("artifacts")
        .and_then(Value::as_array)
        .is_some_and(|artifacts| !artifacts.is_empty())
}

fn execute_catalog_attribution(
    device: &mut impl BenchmarkDevice,
    manifest: String,
    output_dir: PathBuf,
    reporter: &mut Reporter<'_>,
    profile: BenchmarkProfile,
    arm: &str,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        &format!("capturing catalog {arm} attribution"),
        Some(35),
    )?;
    let detail = device.profile(profile, output_dir.clone())?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    device.verify_health()?;
    evaluate_catalog_attribution_summary(&summary, arm)?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn evaluate_catalog_attribution_summary(summary: &Value, arm: &str) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-catalog-attribution-arm-v1")
        || summary.get("arm").and_then(Value::as_str) != Some(arm)
        || summary.get("status").and_then(Value::as_str) != Some("passed")
    {
        return Err(format!("catalog {arm} attribution is not a passing v1 arm").into());
    }
    Ok(())
}

fn evaluate_catalog_lifecycle_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str) != Some("mister-magik-installed-benchmark-v3")
    {
        return Err("catalog lifecycle benchmark summary has the wrong schema".into());
    }
    if summary.get("scenario").and_then(Value::as_str) != Some("catalog-lifecycle") {
        return Err("catalog lifecycle benchmark summary has the wrong scenario".into());
    }
    if summary
        .pointer("/startup_intro/schema")
        .and_then(Value::as_str)
        != Some("mister-magik-startup-intro-qualification-v4")
    {
        return Err("startup intro qualification has the wrong schema".into());
    }
    if summary
        .pointer("/startup_intro/cadence/source")
        .and_then(Value::as_str)
        != Some("fpga-owned-vblank-telemetry")
        || summary
            .pointer("/startup_intro/cadence/presentation_telemetry/available")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err(
            "startup intro qualification has no authoritative FPGA cadence evidence".into(),
        );
    }
    if summary.pointer("/catalog/valid").and_then(Value::as_bool) != Some(true) {
        return Err("catalog lifecycle benchmark did not produce a valid catalog".into());
    }
    let systems = summary
        .pointer("/catalog/systems")
        .and_then(Value::as_array)
        .ok_or("catalog lifecycle benchmark summary has no systems")?;
    if systems.is_empty() {
        return Err("catalog lifecycle benchmark produced no systems".into());
    }
    let dropped_frames = summary
        .pointer("/startup_intro/cadence/dropped_frames")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    if dropped_frames > 0
        || summary
            .pointer("/startup_intro/cadence/qualified")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err(format!(
            "startup intro FAIL — {dropped_frames} dropped frames; cadence={}",
            summary
                .pointer("/startup_intro/cadence")
                .cloned()
                .unwrap_or(Value::Null)
        )
        .into());
    }
    if summary
        .pointer("/startup_intro/latch_protocol/qualified")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err(format!(
            "startup intro latch protocol failed independently of cadence; latch_protocol={}",
            summary
                .pointer("/startup_intro/latch_protocol")
                .cloned()
                .unwrap_or(Value::Null)
        )
        .into());
    }
    Ok(())
}

fn evaluate_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("schema").and_then(Value::as_str)
        != Some("mister-magik-installed-screensaver-benchmark-v7")
    {
        return Err("screensaver benchmark summary has the wrong schema".into());
    }
    let runs = summary
        .get("runs")
        .and_then(Value::as_array)
        .ok_or("screensaver benchmark summary has no runs")?;
    if runs.len() != 2 {
        return Err(format!(
            "screensaver benchmark expected paired cadence/profile runs, received {}",
            runs.len()
        )
        .into());
    }
    if runs[0].get("pass").and_then(Value::as_str) != Some("cadence")
        || runs[0]
            .get("cadence_authoritative")
            .and_then(Value::as_bool)
            != Some(true)
        || runs[1].get("pass").and_then(Value::as_str) != Some("profile")
        || runs[1]
            .get("cadence_authoritative")
            .and_then(Value::as_bool)
            != Some(false)
    {
        return Err("screensaver benchmark pass authority is invalid".into());
    }
    for run in runs {
        evaluate_run(run)?;
    }
    Ok(())
}

fn evaluate_run(run: &Value) -> AgentResult<()> {
    let id = u64_field(run, "run", 0);
    let steady = run
        .get("steady_state")
        .ok_or_else(|| format!("screensaver profile run {id} has no steady-state evidence"))?;
    let physical = steady
        .get("physical_refresh")
        .ok_or_else(|| format!("screensaver profile run {id} has no physical refresh evidence"))?;
    let unique_fps = physical
        .get("unique_fps")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let refresh_hz = physical
        .get("refresh_hz")
        .and_then(Value::as_f64)
        .unwrap_or(f64::INFINITY);
    let dropped_frames = u64_field(physical, "dropped_frames", u64::MAX);
    let authoritative = physical.get("source").and_then(Value::as_str)
        == Some("fpga-owned-vblank-telemetry")
        && physical
            .pointer("/presentation_telemetry/schema")
            .and_then(Value::as_str)
            == Some("mister-magik-frame-evidence-v6");
    let frames = u64_field(steady, "frames", 0);
    let p99_work = u64_field(steady, "p99_work_us", u64::MAX);
    let p99_wall = u64_field(steady, "p99_wall_us", u64::MAX);
    let max_wall = u64_field(steady, "max_wall_us", u64::MAX);
    let refresh = u64_field(steady, "refresh_period_us", 16_667);
    let over_budget = u64_field(steady, "over_budget_frames", u64::MAX);
    let presentation_failures = steady
        .get("presentation_failures")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(usize::MAX);
    let latch_drops = u64_field(run, "latch_drop_delta", u64::MAX);
    let misses = u64_field(steady, "vsync_misses", u64::MAX);
    let errors = u64_field(run, "present_errors", u64::MAX);
    let status = run
        .get("status_publishing")
        .ok_or_else(|| format!("screensaver profile run {id} has no status publishing evidence"))?;
    let status_mode = status
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let status_enqueue_p99 = u64_field(status, "enqueue_p99_us", u64::MAX);
    let status_worker_errors = u64_field(status, "worker_errors", u64::MAX);
    let status_submitted = u64_field(status, "final_submitted_sequence", 0);
    let status_written = u64_field(status, "final_written_sequence", 0);
    if !authoritative
        || unique_fps < refresh_hz - 0.1
        || dropped_frames != 0
        || frames == 0
        || presentation_failures != 0
        || latch_drops != 0
        || misses != 0
        || errors != 0
        || status_mode != "async"
        || status_enqueue_p99 >= 250
        || status_worker_errors != 0
        || status_submitted == 0
        || status_written != status_submitted
    {
        return Err(format!(
            "screensaver profile run {id} FAIL — {dropped_frames} dropped frames after warm-up: authoritative={authoritative} frames={frames} unique_fps={unique_fps:.2}/{refresh_hz:.2} presentation_failures={presentation_failures} timing_overruns={over_budget} p99_work_us={p99_work} p99_wall_us={p99_wall} max_wall_us={max_wall} refresh_period_us={refresh} latch_drops={latch_drops} vsync_misses={misses} present_errors={errors} status_mode={status_mode} status_enqueue_p99_us={status_enqueue_p99} status_worker_errors={status_worker_errors} status_sequences={status_submitted}/{status_written}"
        )
        .into());
    }
    Ok(())
}

fn u64_field(value: &Value, field: &str, default: u64) -> u64 {
    value.get(field).and_then(Value::as_u64).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_input_requires_held_release_isolation_and_a_fresh_activation() {
        let passing = json!({
            "schema": "mister-magik-modal-input-verification-v1",
            "status": "passed",
            "during_hold": {"return_screen": "home"},
            "after_release": {"return_screen": "home"},
            "fresh_press": {"return_screen": "arcade"},
            "rebuild_selected": {"dialog_selected": 1},
        });
        assert!(evaluate_modal_input_summary(&passing).is_ok());

        for pointer in ["during_hold", "after_release"] {
            let mut leaked = passing.clone();
            leaked[pointer]["return_screen"] = json!("arcade");
            assert!(evaluate_modal_input_summary(&leaked).is_err());
        }
        let mut no_rebuild_selection = passing.clone();
        no_rebuild_selection["rebuild_selected"]["dialog_selected"] = json!(0);
        assert!(evaluate_modal_input_summary(&no_rebuild_selection).is_err());
        let mut no_fresh_activation = passing;
        no_fresh_activation["fresh_press"]["return_screen"] = json!("home");
        assert!(evaluate_modal_input_summary(&no_fresh_activation).is_err());
    }

    fn passing_run(run: u64, pass: &str, cadence_authoritative: bool) -> Value {
        json!({
            "run": run,
            "pass": pass,
            "cadence_authoritative": cadence_authoritative,
            "startup": {
                "ignored_frames": 3,
                "max_wall_us": 500_000
            },
            "steady_state": {
                "frames": 1_797,
                "average_fps": 59.9,
                "p99_work_us": 10_000,
                "p99_wall_us": 16_000,
                "max_wall_us": 16_667,
                "refresh_period_us": 16_667,
                "over_budget_frames": 0,
                "vsync_misses": 0,
                "presentation_failures": [],
                "physical_refresh": {
                    "source": "fpga-owned-vblank-telemetry",
                    "refresh_hz": 60.0,
                    "unique_fps": 60.0,
                    "dropped_frames": 0,
                    "presentation_telemetry": {
                        "schema": "mister-magik-frame-evidence-v6"
                    },
                    "software_refresh_diagnostics": {
                        "long_completion_intervals": []
                    }
                }
            },
            "latch_drop_delta": 0,
            "present_errors": 0,
            "status_publishing": {
                "mode": "async",
                "enqueue_p99_us": 100,
                "worker_errors": 0,
                "final_submitted_sequence": 31,
                "final_written_sequence": 31
            },
        })
    }

    #[test]
    fn installed_screensaver_requires_paired_passing_runs() {
        assert!(
            evaluate_summary(&json!({
                "schema": "mister-magik-installed-screensaver-benchmark-v7",
                "runs": [
                    passing_run(1, "cadence", true),
                    passing_run(2, "profile", false),
                ]
            }))
            .is_ok()
        );
        assert!(
            evaluate_summary(&json!({
                "schema": "mister-magik-installed-screensaver-benchmark-v7",
                "runs": [passing_run(1, "cadence", true)]
            }))
            .is_err()
        );
        assert!(
            evaluate_summary(&json!({
                "schema": "mister-magik-installed-screensaver-benchmark-v6",
                "runs": [
                    passing_run(1, "cadence", true),
                    passing_run(2, "profile", false),
                ]
            }))
            .is_err()
        );
    }

    #[test]
    fn particle_benchmarks_redirect_to_the_dedicated_scene_lab() {
        for scenario in [
            BenchmarkScenario::Particles,
            BenchmarkScenario::ParticleCapacity,
            BenchmarkScenario::ParticleDemo40k,
            BenchmarkScenario::ParticleStep,
            BenchmarkScenario::ParticleProfile,
        ] {
            let command = particle_scene_lab_command(scenario).expect("particle lab command");
            assert!(command.contains("device scene-lab"));
            assert!(command.contains("--scene magik"));
            assert!(command.contains("--particle-preset"));
            assert!(command.contains("--particle-count"));
        }
    }

    #[test]
    fn pmu_suite_requires_four_passing_workloads() {
        let passing = json!({
            "schema": "mister-magik-pmu-suite-v2",
            "status": "passed",
            "workloads": [
                {"workload": "probe", "status": "ok"},
                {"workload": "screensaver", "status": "ok"},
                {"workload": "search", "status": "ok"},
                {"workload": "catalog", "status": "ok"},
            ],
        });
        evaluate_pmu_summary(&passing).unwrap();
        let mut failed = passing;
        failed["workloads"][1]["status"] = json!("failed");
        assert!(evaluate_pmu_summary(&failed).is_err());
    }

    #[test]
    fn input_integrity_requires_every_zero_loss_gate() {
        let passing = json!({
            "schema": "mister-magik-input-integrity-v2",
            "status": "passed",
            "protocol": 2,
            "lost_actions": 0,
            "duplicated_actions": 0,
            "proxy_write_failures": 0,
            "journal_overflows": 0,
            "sequence_gaps": 0,
        });
        evaluate_input_integrity_summary(&passing).unwrap();
        let mut proxy_v3 = passing.clone();
        proxy_v3["protocol"] = json!(3);
        evaluate_input_integrity_summary(&proxy_v3).unwrap();
        let mut unsupported = passing.clone();
        unsupported["protocol"] = json!(4);
        assert!(evaluate_input_integrity_summary(&unsupported).is_err());
        let mut failed = passing;
        failed["lost_actions"] = json!(1);
        assert!(evaluate_input_integrity_summary(&failed).is_err());
    }

    #[test]
    fn launcher_response_requires_independent_response_pulse_and_background_gates() {
        let passing = json!({
            "schema": "mister-magik-launcher-response-v2",
            "status": "passed",
            "protocol": 3,
            "input_response_status": "passed",
            "pulse_status": "passed",
            "integrity_status": "passed",
            "background_adoption_status": "passed",
            "dispatch_p95_us": 3_000,
            "dispatch_max_us": 5_000,
            "confirmed_median_us": 12_000,
            "lost_actions": 0,
            "duplicated_actions": 0,
            "coalesced_actions": 0,
            "reordered_actions": 0,
            "proxy_write_failures": 0,
            "journal_overflows": 0,
            "sequence_gaps": 0,
            "latch_drops": 0,
            "repeated_vblanks": 0,
            "ownership_losses": 0,
            "catalog_adoption_max_us": 7_999,
        });
        evaluate_launcher_response_summary(&passing).unwrap();
        let mut unsupported_protocol = passing.clone();
        unsupported_protocol["protocol"] = json!(1);
        assert!(evaluate_launcher_response_summary(&unsupported_protocol).is_err());
        let mut failed = passing;
        failed["pulse_status"] = json!("failed");
        assert!(evaluate_launcher_response_summary(&failed).is_err());
    }

    #[test]
    fn isolated_launcher_response_accepts_idle_background_and_static_vblanks() {
        let mut summary = json!({
            "schema": "mister-magik-launcher-response-v2",
            "status": "passed",
            "protocol": 2,
            "diagnostic_mode": "1920x1200-200-300-400-600ms-two-round-trips",
            "input_response_status": "passed",
            "pulse_status": "passed",
            "integrity_status": "passed",
            "background_adoption_applicable": false,
            "background_adoption_status": "not-applicable",
            "dispatch_p95_us": 3_000,
            "dispatch_max_us": 5_000,
            "confirmed_median_us": 12_000,
            "lost_actions": 0,
            "duplicated_actions": 0,
            "coalesced_actions": 0,
            "reordered_actions": 0,
            "proxy_write_failures": 0,
            "journal_overflows": 0,
            "sequence_gaps": 0,
            "latch_drops": 0,
            "repeated_vblanks": 500,
            "ownership_losses": 0,
            "catalog_adoption_max_us": 0,
        });
        evaluate_launcher_response_summary(&summary).unwrap();
        summary.as_object_mut().unwrap().remove("diagnostic_mode");
        assert!(evaluate_launcher_response_summary(&summary).is_err());
    }

    #[test]
    fn input_latency_lab_accepts_complete_evidence_despite_failed_quality() {
        let mut summary = json!({
            "schema": "mister-magik-input-latency-lab-v1",
            "status": "completed",
            "artifact_validity_status": "passed",
            "current_product_quality_status": "failed",
            "arms": [{}, {}, {}, {}, {}, {}, {}, {}, {}],
        });
        evaluate_input_latency_lab_summary(&summary).unwrap();
        summary["arms"] = json!([{}, {}, {}, {}, {}, {}, {}, {}]);
        assert!(evaluate_input_latency_lab_summary(&summary).is_err());
        summary["arms"] = json!([{}, {}, {}, {}, {}, {}, {}, {}, {}]);
        summary["artifact_validity_status"] = json!("failed");
        assert!(evaluate_input_latency_lab_summary(&summary).is_err());
    }

    #[test]
    fn installed_screensaver_rejects_performance_or_platform_errors() {
        let mut slow = passing_run(1, "cadence", true);
        slow["steady_state"]["physical_refresh"]["unique_fps"] = json!(40.0);
        assert!(evaluate_run(&slow).is_err());
        let mut latch_dropped = passing_run(1, "cadence", true);
        latch_dropped["latch_drop_delta"] = json!(1);
        assert!(evaluate_run(&latch_dropped).is_err());
        let mut late_start = passing_run(1, "cadence", true);
        late_start["startup"]["max_wall_us"] = json!(5_000_000);
        assert!(evaluate_run(&late_start).is_ok());
        let mut timing_overrun = passing_run(1, "cadence", true);
        timing_overrun["steady_state"]["over_budget_frames"] = json!(1);
        assert!(evaluate_run(&timing_overrun).is_ok());
        let mut presentation_failure = passing_run(1, "cadence", true);
        presentation_failure["steady_state"]["presentation_failures"] =
            json!([{"frame": 42, "kind": "sequence-gap"}]);
        assert!(evaluate_run(&presentation_failure).is_err());
        let mut dropped = passing_run(1, "cadence", true);
        dropped["steady_state"]["physical_refresh"]["dropped_frames"] = json!(1);
        assert!(evaluate_run(&dropped).is_err());
        let mut legacy = passing_run(1, "cadence", true);
        legacy["steady_state"]["physical_refresh"]
            .as_object_mut()
            .unwrap()
            .remove("dropped_frames");
        // dropped-frame-legacy-fixture: old evidence must fail closed.
        legacy["steady_state"]["physical_refresh"]["repeated_refreshes"] = json!(0);
        assert!(evaluate_run(&legacy).is_err());
        let mut long_gap = passing_run(1, "cadence", true);
        long_gap["steady_state"]["physical_refresh"]["software_refresh_diagnostics"]["long_completion_intervals"] =
            json!([{"frame": 42, "interval_us": 33_334}]);
        assert!(evaluate_run(&long_gap).is_ok());
        let mut blocking_status = passing_run(1, "cadence", true);
        blocking_status["status_publishing"]["enqueue_p99_us"] = json!(250);
        assert!(evaluate_run(&blocking_status).is_err());
    }

    #[test]
    fn catalog_lifecycle_requires_a_valid_nonempty_catalog() {
        let passing = json!({
            "schema": "mister-magik-installed-benchmark-v3",
            "scenario": "catalog-lifecycle",
            "catalog": {
                "valid": true,
                "systems": [{"system": "atari2600", "games": 2}]
            },
            "startup_intro": {
                "schema": "mister-magik-startup-intro-qualification-v4",
                "cadence": {
                    "qualified": true,
                    "dropped_frames": 0,
                    "source": "fpga-owned-vblank-telemetry",
                    "presentation_telemetry": {"available": true}
                },
                "latch_protocol": {"qualified": true}
            }
        });
        assert!(evaluate_catalog_lifecycle_summary(&passing).is_ok());

        let mut legacy_installed = passing.clone();
        legacy_installed["schema"] = json!("mister-magik-installed-benchmark-v2");
        assert!(evaluate_catalog_lifecycle_summary(&legacy_installed).is_err());

        let mut legacy = passing.clone();
        legacy["startup_intro"]["schema"] = json!("mister-magik-startup-intro-qualification-v3");
        assert!(evaluate_catalog_lifecycle_summary(&legacy).is_err());

        let mut invalid = passing.clone();
        invalid["catalog"]["valid"] = json!(false);
        assert!(evaluate_catalog_lifecycle_summary(&invalid).is_err());

        let mut empty = passing;
        empty["catalog"]["systems"] = json!([]);
        assert!(evaluate_catalog_lifecycle_summary(&empty).is_err());

        let mut dropped = json!({
            "schema": "mister-magik-installed-benchmark-v3",
            "scenario": "catalog-lifecycle",
            "catalog": {"valid": true, "systems": [{"system": "arcade"}]},
            "startup_intro": {
                "schema": "mister-magik-startup-intro-qualification-v4",
                "cadence": {
                    "qualified": false,
                    "dropped_frames": 1,
                    "source": "fpga-owned-vblank-telemetry",
                    "presentation_telemetry": {"available": true}
                },
                "latch_protocol": {"qualified": true, "latch_drop_delta": 0}
            }
        });
        assert!(evaluate_catalog_lifecycle_summary(&dropped).is_err());
        dropped["startup_intro"]["cadence"]["qualified"] = json!(true);
        dropped["startup_intro"]["latch_protocol"]["qualified"] = json!(false);
        assert!(evaluate_catalog_lifecycle_summary(&dropped).is_err());
    }

    #[test]
    fn catalog_build_rebuild_requires_three_passing_delta_samples() {
        let catalog = || {
            json!({
                "valid": true,
                "identity_sha256": "1111111111111111111111111111111111111111111111111111111111111111",
                "ordering_sha256": "2222222222222222222222222222222222222222222222222222222222222222",
                "launch_sha256": "3333333333333333333333333333333333333333333333333333333333333333",
                "search_sha256": "4444444444444444444444444444444444444444444444444444444444444444",
                "artifact_set_sha256": "5555555555555555555555555555555555555555555555555555555555555555",
                "artifacts": [{"sha256": "6666666666666666666666666666666666666666666666666666666666666666"}],
            })
        };
        let sample = || {
            json!({
                "status": "passed",
                "fresh": {"catalog": catalog()},
                "rebuild": {"catalog": catalog()},
                "validation": {"snes_game_delta": 1},
            })
        };
        let passing = json!({
            "schema": "mister-magik-catalog-build-rebuild-v3",
            "scenario": "catalog-build-rebuild",
            "status": "passed",
            "configuration": {"unmeasured_warmups": 1},
            "production_registry": {"unchanged": true},
            "samples": [sample(), sample(), sample()],
        });
        assert!(evaluate_catalog_build_rebuild_summary(&passing).is_ok());
        let mut failed = passing;
        failed["samples"][1]["validation"]["snes_game_delta"] = json!(0);
        assert!(evaluate_catalog_build_rebuild_summary(&failed).is_err());
    }

    #[test]
    fn whole_card_catalog_requires_matching_valid_counts() {
        let catalog = || {
            json!({
                "valid": true,
                "identity_sha256": "1111111111111111111111111111111111111111111111111111111111111111",
                "ordering_sha256": "2222222222222222222222222222222222222222222222222222222222222222",
                "launch_sha256": "3333333333333333333333333333333333333333333333333333333333333333",
                "search_sha256": "4444444444444444444444444444444444444444444444444444444444444444",
                "artifact_set_sha256": "5555555555555555555555555555555555555555555555555555555555555555",
                "artifacts": [{"sha256": "6666666666666666666666666666666666666666666666666666666666666666"}],
            })
        };
        let passing = json!({
            "schema": "mister-magik-catalog-full-build-rebuild-v3",
            "scenario": "catalog-full-build-rebuild",
            "status": "passed",
            "first_observed_clean": {"catalog": catalog()},
            "warm_clean": {"catalog": catalog()},
            "rebuild": {"catalog": catalog()},
            "validation": {
                "exact_identities_identical": true,
                "artifact_sets_valid": true,
                "phase_evidence_complete": true
            },
        });
        assert!(evaluate_catalog_full_build_rebuild_summary(&passing).is_ok());
        let mut failed = passing;
        failed["validation"]["exact_identities_identical"] = json!(false);
        assert!(evaluate_catalog_full_build_rebuild_summary(&failed).is_err());
    }

    #[test]
    fn launch_return_requires_two_restored_cycles_with_authoritative_timestamps() {
        let cycle = json!({
            "capsule_fault_injected": false,
            "restored": true,
            "black_interval_ms": 5,
            "total_return_us": 5_000,
            "request_monotonic_us": 10_000,
            "acknowledged_monotonic_us": 10_100,
            "process_start_monotonic_us": 11_000,
            "exact_context_monotonic_us": 12_000,
            "preview_ready_monotonic_us": 14_000,
            "first_correct_present_monotonic_us": 15_000,
            "preview_verified": true,
            "cpu_sample_hits": 10,
            "cpu_sample_stacks": 2,
            "resolved_application_symbols": true,
            "capture_file": "capture.png",
            "capture_metadata_file": "capture.json",
            "flamegraph_file": "flamegraph.svg",
            "folded_stacks_file": "stacks.folded",
            "cpu_profile_file": "profile.json",
            "frame_profile_file": "frames.tsv",
            "timeline_file": "timeline.json",
        });
        let passing = json!({
            "schema": "mister-magik-launch-return-benchmark-v3",
            "scenario": "launch-return",
            "timing_class": "instrumented-installed-dev-symbols",
            "main_revision": "a".repeat(40),
            "main_sha256": "b".repeat(64),
            "magik_revision": "c".repeat(40),
            "gui_sha256": "d".repeat(64),
            "latency": {
                "command_to_process": {"min_us": 1, "median_us": 2, "max_us": 3},
                "process_to_context": {"min_us": 1, "median_us": 2, "max_us": 3},
                "context_to_preview": {"min_us": 1, "median_us": 2, "max_us": 3},
                "preview_to_present": {"min_us": 1, "median_us": 2, "max_us": 3},
                "total_return": {"min_us": 1, "median_us": 2, "max_us": 3}
            },
            "cycles": [cycle.clone(), cycle]
        });
        evaluate_launch_return_summary(&passing, "launch-return", false).unwrap();
        let mut slow = passing.clone();
        slow["cycles"][1]["black_interval_ms"] = json!(5_000);
        slow["cycles"][1]["total_return_us"] = json!(5_000_000);
        slow["cycles"][1]["first_correct_present_monotonic_us"] = json!(5_010_000);
        assert!(evaluate_launch_return_summary(&slow, "launch-return", false).is_err());
        let mut unrestored = passing.clone();
        unrestored["cycles"][0]["restored"] = json!(false);
        assert!(evaluate_launch_return_summary(&unrestored, "launch-return", false).is_err());
        let mut zero = passing.clone();
        zero["cycles"][0]["request_monotonic_us"] = json!(0);
        assert!(evaluate_launch_return_summary(&zero, "launch-return", false).is_err());
        let mut unordered = passing.clone();
        unordered["cycles"][0]["acknowledged_monotonic_us"] = json!(11_500);
        assert!(evaluate_launch_return_summary(&unordered, "launch-return", false).is_err());
        assert!(
            evaluate_launch_return_summary(
                &json!({"schema": "wrong", "cycles": []}),
                "launch-return",
                false
            )
            .is_err()
        );

        let mut fallback = passing;
        fallback["scenario"] = json!("launch-return-fallback");
        fallback["cycles"][1]["capsule_fault_injected"] = json!(true);
        fallback["cycles"][1]["return_source"] = json!("sharded-registry");
        evaluate_launch_return_summary(&fallback, "launch-return-fallback", true).unwrap();
    }

    #[test]
    fn launch_return_health_follows_owned_cleanup() {
        assert!(!benchmark_requires_initial_health(
            BenchmarkScenario::LaunchReturn
        ));
        assert!(!benchmark_requires_initial_health(
            BenchmarkScenario::LaunchReturnFallback
        ));
        assert!(benchmark_requires_initial_health(
            BenchmarkScenario::Screensaver
        ));
    }

    #[test]
    fn benchmark_rejects_public_or_suspended_runtime_with_remediation() {
        for active in [
            crate::host::ActiveRuntime::new(
                Some("/media/fat/MiSTer_MagiK"),
                Some("LauncherActive"),
            ),
            crate::host::ActiveRuntime::new(
                Some("/media/fat/MiSTer_MagiKDev"),
                Some("LauncherSuspended"),
            ),
        ] {
            let error = require_active_development_runtime(&active).unwrap_err();
            assert!(error.to_string().contains(&active.description()));
            assert!(error.to_string().contains("scripts/agent deliver"));
        }
        assert!(
            require_active_development_runtime(&crate::host::ActiveRuntime::new(
                Some("/media/fat/MiSTer_MagiKDev"),
                Some("LauncherActive"),
            ))
            .is_ok()
        );
    }

    #[test]
    fn cold_boot_requires_ordered_device_timing_and_visible_capture() {
        let passing = json!({
            "schema": "mister-magik-cold-boot-benchmark-v1",
            "timing_class": "device-monotonic-instrumented-installed-dev",
            "launcher_ready": true,
            "capture_verified": true,
            "fresh_catalog": false,
            "timeline": {
                "agent_start_us": 1,
                "initial_main_entry_us": 2,
                "final_main_entry_us": 3,
                "preflight_begin_us": 4,
                "preflight_end_us": 5,
                "launcher_exec_us": 6,
                "magik_process_start_us": 7,
                "first_launcher_present_us": 8,
            }
        });
        evaluate_cold_boot_summary(&passing, false, false).unwrap();

        let mut unordered = passing.clone();
        unordered["timeline"]["launcher_exec_us"] = json!(9);
        assert!(evaluate_cold_boot_summary(&unordered, false, false).is_err());

        let mut missing_capture = passing.clone();
        missing_capture["capture_verified"] = json!(false);
        assert!(evaluate_cold_boot_summary(&missing_capture, false, false).is_err());

        let mut profiled = passing.clone();
        profiled["profile"] = json!({"state": "complete", "sample_hits": 42});
        evaluate_cold_boot_summary(&profiled, true, false).unwrap();
        profiled["profile"]["sample_hits"] = json!(0);
        assert!(evaluate_cold_boot_summary(&profiled, true, false).is_err());

        let mut fresh = passing;
        fresh["fresh_catalog"] = json!(true);
        fresh["timeline"]["magik_startup_events"] = json!([{
            "event": "startup_entry_classified",
            "detail": "mode=cold_no_catalog",
        }]);
        evaluate_cold_boot_summary(&fresh, false, true).unwrap();
        fresh["timeline"]["magik_startup_events"] = json!([]);
        assert!(evaluate_cold_boot_summary(&fresh, false, true).is_err());
    }

    #[test]
    fn search_evaluators_require_timing_and_ready_results() {
        let query = json!({
            "query": "A",
            "result_hash": "0".repeat(64),
        });
        let run = json!({
            "queries": [query.clone(), query.clone(), query.clone(), query],
        });
        let timing = json!({
            "schema": "mister-magik-search-benchmark-v2",
            "runs": [run.clone(), run.clone(), run],
            "warm_all_queries": {"total_us": {"p95": 1}}
        });
        evaluate_search_summary(&timing).unwrap();
        let mut no_queries = timing.clone();
        no_queries["runs"][0]["queries"] = json!([]);
        assert!(evaluate_search_summary(&no_queries).is_err());
        let mut no_timing = timing;
        no_timing["warm_all_queries"] = Value::Null;
        assert!(evaluate_search_summary(&no_timing).is_err());

        let ui = json!({
            "schema": "mister-magik-search-ui-verification-v1",
            "status": "ready",
            "query": "A",
            "results": 1,
        });
        evaluate_search_ui_summary(&ui).unwrap();
        for field in ["schema", "status", "query", "results"] {
            let mut invalid = ui.clone();
            invalid[field] = Value::Null;
            assert!(evaluate_search_ui_summary(&invalid).is_err());
        }
    }

    #[test]
    fn rom_identity_evaluator_requires_production_default_cases() {
        let arm = json!({
            "case_count": 3,
            "production_default_selected": 1,
            "implementation": "streaming-slicing-by-eight-crc32",
        });
        let passing = json!({
            "schema": "mister-magik-rom-identity-production-v1",
            "status": "passed",
            "runs": [arm.clone(), arm.clone(), arm],
        });
        evaluate_rom_identity_hashing_summary(&passing).unwrap();
        for field in ["schema", "status", "runs"] {
            let mut invalid = passing.clone();
            invalid[field] = Value::Null;
            assert!(evaluate_rom_identity_hashing_summary(&invalid).is_err());
        }
    }

    #[test]
    fn media_pack_persistence_evaluator_requires_production_streaming_rows() {
        let passing = json!({
            "schema": "mister-magik-media-pack-persistence-v1",
            "status": "passed",
            "save_strategy": "stream-fat",
            "decode_ms": 0,
            "row_count": 9,
        });
        evaluate_media_pack_persistence_summary(&passing).unwrap();
        for field in [
            "schema",
            "status",
            "save_strategy",
            "decode_ms",
            "row_count",
        ] {
            let mut invalid = passing.clone();
            invalid[field] = Value::Null;
            assert!(evaluate_media_pack_persistence_summary(&invalid).is_err());
        }
    }
}
