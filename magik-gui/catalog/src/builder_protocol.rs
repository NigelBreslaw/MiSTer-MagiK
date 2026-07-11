use serde::{Deserialize, Serialize};

pub const CATALOG_BUILDER_PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_CATALOG_BUILDER_LOCK_PATH: &str =
    "/tmp/mister-magik/catalog-builder.lock";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CatalogBuilderEvent {
    Handshake {
        protocol: u32,
        operation: String,
        run_id: String,
    },
    Progress {
        protocol: u32,
        title: String,
        detail: String,
    },
    SystemDiscovered {
        protocol: u32,
        system_id: String,
    },
    Timing {
        protocol: u32,
        name: String,
        detail: String,
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
    },
    Failure {
        protocol: u32,
        stage: String,
        error: String,
    },
    Done {
        protocol: u32,
    },
}

impl CatalogBuilderEvent {
    pub fn protocol(&self) -> u32 {
        match self {
            Self::Handshake { protocol, .. }
            | Self::Progress { protocol, .. }
            | Self::SystemDiscovered { protocol, .. }
            | Self::Timing { protocol, .. }
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
        };
        let encoded = serde_json::to_string(&event).unwrap();
        assert_eq!(
            serde_json::from_str::<CatalogBuilderEvent>(&encoded).unwrap(),
            event
        );
    }
}
