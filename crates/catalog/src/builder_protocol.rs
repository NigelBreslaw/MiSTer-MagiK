// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::{Deserialize, Serialize};

// Keep this compatible with the previous builder during an in-place deploy. The
// new operation and events are additive; an old launcher never requests them.
pub const CATALOG_BUILDER_PROTOCOL_VERSION: u32 = 4;
pub const DEFAULT_CATALOG_BUILDER_LOCK_PATH: &str = "/tmp/mister-magik/catalog-builder.lock";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CatalogChangeReason {
    InputsChanged,
    ProjectionUpgrade { installed: String, required: String },
    RepairRequired,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogFailureCode {
    UnsupportedSchema,
    CorruptCatalog,
    Io,
    ResourceLimit,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct CatalogFailureDiagnostic {
    pub code: CatalogFailureCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CatalogScanTargetProgress {
    pub ordinal: usize,
    pub total: usize,
    pub path: String,
    pub target_kind: String,
    pub state: String,
    pub completed_targets: usize,
    pub discoveries: usize,
    pub execution_mode: String,
    pub cooperative_policy: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct CatalogProgressMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_target: Option<CatalogScanTargetProgress>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogPlannedAction {
    Rebuild,
    Remove,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogPlanReason {
    ChangedInput,
    ConservativeFallback,
    RemovedInput,
    ProjectionUpgrade,
    ExplicitRebuild,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CatalogPlannedSystem {
    pub system_id: String,
    pub action: CatalogPlannedAction,
    pub reasons: Vec<CatalogPlanReason>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CatalogBuilderEvent {
    Handshake {
        protocol: u32,
        operation: String,
        run_id: String,
    },
    Heartbeat {
        protocol: u32,
        run_id: String,
        phase: String,
        sequence: u64,
        progress_epoch: u64,
        work_units: u64,
    },
    Progress {
        protocol: u32,
        title: String,
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<CatalogProgressMetadata>,
    },
    PlanReady {
        protocol: u32,
        system_ids: Vec<String>,
        all_published_systems: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        systems: Vec<CatalogPlannedSystem>,
    },
    SystemDiscovered {
        protocol: u32,
        system_id: String,
    },
    SystemScanning {
        protocol: u32,
        system_id: String,
    },
    SystemPrepared {
        protocol: u32,
        system_id: String,
        generation: u64,
    },
    SystemFailed {
        protocol: u32,
        system_id: String,
        stage: String,
        error: String,
    },
    ManifestPublished {
        protocol: u32,
        generation: u64,
        rebuilt: Vec<String>,
        removed: Vec<String>,
    },
    Timing {
        protocol: u32,
        name: String,
        detail: String,
    },
    FreshCleanupStarted {
        protocol: u32,
    },
    FreshCleanupCompleted {
        protocol: u32,
        removed: usize,
    },
    CatalogReady {
        protocol: u32,
        snapshot_path: String,
        games: usize,
        load_us: u64,
    },
    Persisted {
        protocol: u32,
        summary: BuilderSummary,
    },
    Unchanged {
        protocol: u32,
        summary: BuilderSummary,
    },
    Changed {
        protocol: u32,
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<CatalogChangeReason>,
    },
    Failure {
        protocol: u32,
        stage: String,
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diagnostic: Option<CatalogFailureDiagnostic>,
    },
    Done {
        protocol: u32,
    },
}

impl CatalogBuilderEvent {
    pub fn protocol(&self) -> u32 {
        match self {
            Self::Handshake { protocol, .. }
            | Self::Heartbeat { protocol, .. }
            | Self::Progress { protocol, .. }
            | Self::PlanReady { protocol, .. }
            | Self::SystemDiscovered { protocol, .. }
            | Self::SystemScanning { protocol, .. }
            | Self::SystemPrepared { protocol, .. }
            | Self::SystemFailed { protocol, .. }
            | Self::ManifestPublished { protocol, .. }
            | Self::Timing { protocol, .. }
            | Self::FreshCleanupStarted { protocol }
            | Self::FreshCleanupCompleted { protocol, .. }
            | Self::CatalogReady { protocol, .. }
            | Self::Persisted { protocol, .. }
            | Self::Unchanged { protocol, .. }
            | Self::Changed { protocol, .. }
            | Self::Failure { protocol, .. }
            | Self::Done { protocol } => *protocol,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct BuilderSummary {
    #[serde(default)]
    pub completed_build_seconds: Option<u64>,
    pub skipped: bool,
    pub scan_us: u64,
    pub discover_us: u64,
    pub classify_us: u64,
    pub import_us: u64,
    pub bytes: u64,
    pub normal_files: usize,
    pub containers: usize,
    pub entries: usize,
    pub audit_rows: usize,
    pub discoveries: usize,
}

impl From<crate::library_db::LibraryRefreshSummary> for BuilderSummary {
    fn from(value: crate::library_db::LibraryRefreshSummary) -> Self {
        Self {
            completed_build_seconds: None,
            skipped: value.skipped,
            scan_us: value.scan_us,
            discover_us: value.discover_us,
            classify_us: value.classify_us,
            import_us: value.import_us,
            bytes: value.bytes,
            normal_files: value.normal_files,
            containers: value.containers,
            entries: value.entries,
            audit_rows: value.audit_rows,
            discoveries: value.discoveries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_round_trips_events() {
        let event = CatalogBuilderEvent::Progress {
            protocol: CATALOG_BUILDER_PROTOCOL_VERSION,
            title: "Finding games".into(),
            detail: "42 games found".into(),
            metadata: None,
        };
        let encoded = serde_json::to_string(&event).unwrap();
        assert_eq!(
            serde_json::from_str::<CatalogBuilderEvent>(&encoded).unwrap(),
            event
        );
    }

    #[test]
    fn heartbeat_round_trips_without_being_progress() {
        let event = CatalogBuilderEvent::Heartbeat {
            protocol: CATALOG_BUILDER_PROTOCOL_VERSION,
            run_id: "run-1".into(),
            phase: "scan".into(),
            sequence: 7,
            progress_epoch: 3,
            work_units: 128,
        };
        let encoded = serde_json::to_string(&event).unwrap();
        assert_eq!(
            serde_json::from_str::<CatalogBuilderEvent>(&encoded).unwrap(),
            event
        );
    }

    #[test]
    fn progress_metadata_is_additive_and_round_trips_target_state() {
        let previous: CatalogBuilderEvent = serde_json::from_str(
            r#"{"event":"progress","protocol":1,"title":"Scanning","detail":"working"}"#,
        )
        .unwrap();
        assert!(matches!(
            previous,
            CatalogBuilderEvent::Progress { metadata: None, .. }
        ));

        let event = CatalogBuilderEvent::Progress {
            protocol: CATALOG_BUILDER_PROTOCOL_VERSION,
            title: "Scanning library".into(),
            detail: "target=1 of 170 state=started".into(),
            metadata: Some(CatalogProgressMetadata {
                scan_target: Some(CatalogScanTargetProgress {
                    ordinal: 0,
                    total: 170,
                    path: "/media/fat/games/NES".into(),
                    target_kind: "static".into(),
                    state: "started".into(),
                    completed_targets: 0,
                    discoveries: 0,
                    execution_mode: "background_interactive".into(),
                    cooperative_policy: "continuous_cpu0".into(),
                }),
            }),
        };
        let encoded = serde_json::to_string(&event).unwrap();
        assert_eq!(
            serde_json::from_str::<CatalogBuilderEvent>(&encoded).unwrap(),
            event
        );
    }

    #[test]
    fn affected_system_plan_round_trips_actions_and_reasons() {
        let event = CatalogBuilderEvent::PlanReady {
            protocol: CATALOG_BUILDER_PROTOCOL_VERSION,
            system_ids: vec!["snes".into()],
            all_published_systems: false,
            systems: vec![CatalogPlannedSystem {
                system_id: "snes".into(),
                action: CatalogPlannedAction::Rebuild,
                reasons: vec![CatalogPlanReason::ChangedInput],
            }],
        };
        let encoded = serde_json::to_string(&event).unwrap();
        assert_eq!(
            serde_json::from_str::<CatalogBuilderEvent>(&encoded).unwrap(),
            event
        );

        let previous: CatalogBuilderEvent = serde_json::from_str(
            r#"{"event":"plan_ready","protocol":2,"system_ids":["snes"],"all_published_systems":false}"#,
        )
        .unwrap();
        assert!(matches!(
            previous,
            CatalogBuilderEvent::PlanReady { systems, .. } if systems.is_empty()
        ));
    }

    #[test]
    fn persisted_event_round_trips_completed_build_seconds() {
        let event = CatalogBuilderEvent::Persisted {
            protocol: CATALOG_BUILDER_PROTOCOL_VERSION,
            summary: BuilderSummary {
                completed_build_seconds: Some(119),
                ..BuilderSummary::default()
            },
        };
        let encoded = serde_json::to_string(&event).unwrap();

        assert_eq!(
            serde_json::from_str::<CatalogBuilderEvent>(&encoded).unwrap(),
            event
        );
    }

    #[test]
    fn final_projection_progress_events_round_trip_before_catalog_ready() {
        let protocol = CATALOG_BUILDER_PROTOCOL_VERSION;
        let events = vec![
            CatalogBuilderEvent::Progress {
                protocol,
                title: "Indexing library".into(),
                detail: "Preparing library — 53,458 discoveries".into(),
                metadata: None,
            },
            CatalogBuilderEvent::Progress {
                protocol,
                title: "Indexing library".into(),
                detail: "Resolving playable games — 51,101 of 53,458".into(),
                metadata: None,
            },
            CatalogBuilderEvent::Progress {
                protocol,
                title: "Indexing library".into(),
                detail: "Building launcher indexes — 51,101 of 51,101".into(),
                metadata: None,
            },
            CatalogBuilderEvent::Progress {
                protocol,
                title: "Indexing library".into(),
                detail: "Creating compressed navigation catalog…".into(),
                metadata: None,
            },
            CatalogBuilderEvent::Progress {
                protocol,
                title: "Indexing library".into(),
                detail: "Opening library — 51,101 games".into(),
                metadata: None,
            },
            CatalogBuilderEvent::CatalogReady {
                protocol,
                snapshot_path: "/tmp/catalog.nav.lz4b".into(),
                games: 51_101,
                load_us: 28_000_000,
            },
        ];
        let decoded = events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .map(|line| serde_json::from_str::<CatalogBuilderEvent>(&line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(decoded, events);
        assert!(matches!(
            decoded.last(),
            Some(CatalogBuilderEvent::CatalogReady { .. })
        ));
    }

    #[test]
    fn protocol_accepts_previous_changed_and_failure_shapes() {
        let changed: CatalogBuilderEvent =
            serde_json::from_str(r#"{"event":"changed","protocol":1,"detail":"changed"}"#).unwrap();
        assert!(matches!(
            changed,
            CatalogBuilderEvent::Changed { reason: None, .. }
        ));

        let failure: CatalogBuilderEvent = serde_json::from_str(
            r#"{"event":"failure","protocol":1,"stage":"persist","error":"full"}"#,
        )
        .unwrap();
        assert!(matches!(
            failure,
            CatalogBuilderEvent::Failure {
                diagnostic: None,
                ..
            }
        ));
    }
}
