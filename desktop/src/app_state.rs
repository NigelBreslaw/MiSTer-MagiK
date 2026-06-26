use serde_json::Value;

pub const DEFAULT_HOST: &str = "192.168.1.117";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DashboardSnapshot {
    pub host: String,
    pub connection_state: String,
    pub agent_status: String,
    pub token_source: String,
    pub agent_version: String,
    pub agent_uptime: String,
    pub network_summary: String,
    pub mac_address: String,
    pub main_process: String,
    pub launcher_process: String,
    pub launcher_state: String,
    pub visible_owner: String,
    pub slint_status_freshness: String,
    pub catalog_summary: String,
    pub screen_summary: String,
    pub input_summary: String,
    pub last_error: String,
}

impl DashboardSnapshot {
    pub fn initial(host: impl Into<String>) -> Self {
        let host = host.into();
        Self {
            host: host.clone(),
            connection_state: format!("Looking for MiSTer at {host}"),
            agent_status: "Not checked yet".to_string(),
            token_source: "Not checked yet".to_string(),
            agent_version: "-".to_string(),
            agent_uptime: "-".to_string(),
            network_summary: "ip -; carrier -; state -".to_string(),
            mac_address: "-".to_string(),
            main_process: "-".to_string(),
            launcher_process: "-".to_string(),
            launcher_state: "-".to_string(),
            visible_owner: "-".to_string(),
            slint_status_freshness: "-".to_string(),
            catalog_summary: "-".to_string(),
            screen_summary: "-".to_string(),
            input_summary: "-".to_string(),
            last_error: "".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionOutcome {
    Ready,
    Unauthenticated,
    Unreachable,
    ProtocolError,
}

impl ConnectionOutcome {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ready => "Agent ready",
            Self::Unauthenticated => "Agent unauthenticated",
            Self::Unreachable => "MiSTer unreachable",
            Self::ProtocolError => "Unexpected agent response",
        }
    }
}

pub fn process_summary(value: &Value, name: &str) -> String {
    let Some(list) = value
        .pointer(&format!("/processes/{name}"))
        .and_then(Value::as_array)
    else {
        return "unknown".to_string();
    };
    if list.is_empty() {
        "not running".to_string()
    } else {
        let pids = list
            .iter()
            .filter_map(Value::as_u64)
            .map(|pid| pid.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} running ({pids})", list.len())
    }
}

pub fn string_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

pub fn bool_at(value: &Value, pointer: &str) -> Option<bool> {
    value.pointer(pointer).and_then(Value::as_bool)
}

pub fn number_string_at(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(|v| {
        v.as_i64()
            .map(|n| n.to_string())
            .or_else(|| v.as_u64().map(|n| n.to_string()))
            .or_else(|| v.as_f64().map(|n| format!("{n:.1}")))
    })
}

pub fn uptime_label(ms: Option<u64>) -> String {
    let Some(ms) = ms else {
        return "-".to_string();
    };
    let secs = ms / 1000;
    let minutes = secs / 60;
    let hours = minutes / 60;
    if hours > 0 {
        format!("{hours}h {}m", minutes % 60)
    } else if minutes > 0 {
        format!("{minutes}m {}s", secs % 60)
    } else {
        format!("{secs}s")
    }
}

pub fn catalog_summary(slint_status: &Value) -> String {
    let ready = bool_at(slint_status, "/catalog_ready")
        .map(|v| if v { "ready" } else { "not ready" })
        .unwrap_or("unknown");
    let games = number_string_at(slint_status, "/catalog_games").unwrap_or_else(|| "-".to_string());
    let systems =
        number_string_at(slint_status, "/catalog_systems").unwrap_or_else(|| "-".to_string());
    let scan = string_at(slint_status, "/catalog_scan_message").unwrap_or("");
    if scan.is_empty() {
        format!("{ready}; {games} games; {systems} systems")
    } else {
        format!("{ready}; {games} games; {systems} systems; {scan}")
    }
}

pub fn screen_summary(slint_status: &Value) -> String {
    let screen = string_at(slint_status, "/screen").unwrap_or("unknown");
    let scene = string_at(slint_status, "/scene").unwrap_or("unknown");
    let fps = number_string_at(slint_status, "/rolling_fps")
        .or_else(|| number_string_at(slint_status, "/fps_estimate"))
        .unwrap_or_else(|| "-".to_string());
    let last_frame =
        number_string_at(slint_status, "/last_frame_ms_ago").unwrap_or_else(|| "-".to_string());
    format!("{screen} / {scene}; {fps} fps; last frame {last_frame}ms ago")
}

pub fn input_summary(slint_status: &Value) -> String {
    let pads =
        number_string_at(slint_status, "/input_pad_count").unwrap_or_else(|| "-".to_string());
    let active = string_at(slint_status, "/active_pad_name").unwrap_or("none");
    format!("{pads} pad(s); active: {active}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn process_summary_reports_empty_and_pids() {
        let value = json!({"processes": {"MiSTer_MagiK": [12, 34], "mister-magik-fb": []}});
        assert_eq!(
            process_summary(&value, "MiSTer_MagiK"),
            "2 running (12, 34)"
        );
        assert_eq!(process_summary(&value, "mister-magik-fb"), "not running");
    }

    #[test]
    fn catalog_summary_handles_missing_fields() {
        assert_eq!(catalog_summary(&json!({})), "unknown; - games; - systems");
        assert_eq!(
            catalog_summary(
                &json!({"catalog_ready": true, "catalog_games": 42, "catalog_systems": 3})
            ),
            "ready; 42 games; 3 systems"
        );
    }

    #[test]
    fn uptime_label_formats_short_values() {
        assert_eq!(uptime_label(Some(12_000)), "12s");
        assert_eq!(uptime_label(Some(125_000)), "2m 5s");
        assert_eq!(uptime_label(Some(7_260_000)), "2h 1m");
    }
}
