// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Immutable process-boundary configuration capture.

#[cfg(feature = "ui")]
use crate::launcher_runtime::media::MediaWorkerConfig;
#[cfg(feature = "ui")]
use crate::preview_state::PreviewStateConfig;
#[cfg(feature = "ui")]
use crate::screenshot_transitions::PreviewTransitionConfig;
use mister_magik_catalog::catalog_config::ArchiveCacheConfig;
use mister_magik_catalog::device_layout::{CatalogPathOverrides, CatalogPaths, DevicePaths};
use mister_magik_catalog::fs_fault::FaultConfig;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

const STARTUP_TOKEN: &str = "MISTER_MAGIK_STARTUP_TOKEN";
const READY_FIFO: &str = "MISTER_MAGIK_READY_FIFO";
const MAIN_PID: &str = "MISTER_MAGIK_MAIN_PID";
const MAIN_GENERATION: &str = "MISTER_MAGIK_MAIN_GENERATION";
const OWNER_EPOCH: &str = "MISTER_MAGIK_OWNER_EPOCH";

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
}

#[derive(Clone, Default)]
pub struct LauncherReadinessConfig {
    startup_token: String,
    ready_fifo: PathBuf,
    main_pid: u32,
    main_generation: u64,
    owner_epoch: u64,
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
            main_pid: parse_u32(environment.get(MAIN_PID)),
            main_generation: parse_u64(environment.get(MAIN_GENERATION)),
            owner_epoch: parse_u64(environment.get(OWNER_EPOCH)),
        }
    }

    pub fn into_parts(self) -> (String, PathBuf, u32, u64, u64) {
        (
            self.startup_token,
            self.ready_fifo,
            self.main_pid,
            self.main_generation,
            self.owner_epoch,
        )
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
    fault: Option<FaultConfig>,
}

impl ProcessConfig {
    pub fn capture(args: &[String], command: &str) -> Self {
        Self::from_snapshot_with_device_paths(
            args,
            command,
            &EnvironmentSnapshot::capture_process(),
            DevicePaths::current(),
        )
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
        });
        // Fault capture deliberately remains an early, compatibility-preserving
        // process boundary until C19 applies command and feature gates.
        let fault = FaultConfig::capture_with(|name| environment.get(name));
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
        self.fault.as_ref()
    }
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
                7,
                11,
                0,
            )
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
            &["mister-magik-fb".into(), "catalog-v3-inspect".into()],
            "catalog-v3-inspect",
            &environment,
        );

        assert_eq!(
            config.command(),
            &CommandMode::Other("catalog-v3-inspect".into())
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
}
