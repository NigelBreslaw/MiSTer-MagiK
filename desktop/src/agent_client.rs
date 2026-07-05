use crate::app_state::{
    catalog_summary, input_summary, process_summary, screen_summary, string_at, uptime_label,
    ConnectionOutcome, DashboardSnapshot,
};
use crate::sd_card::{SdDirectoryListing, SdEntry, SdEntryKind};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const AGENT_PORT: u16 = 7498;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenSource {
    Env,
    LocalFile(PathBuf),
    Missing(PathBuf),
}

impl TokenSource {
    pub fn label(&self) -> String {
        match self {
            Self::Env => "MISTER_AGENT_TOKEN".to_string(),
            Self::LocalFile(path) => path.display().to_string(),
            Self::Missing(path) => format!("missing ({})", path.display()),
        }
    }
}

#[derive(Debug)]
pub enum AgentError {
    Unreachable(String),
    Unauthorized,
    Protocol(String),
    Command(String),
}

pub fn read_token() -> (String, TokenSource) {
    if let Ok(token) = env::var("MISTER_AGENT_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return (token, TokenSource::Env);
        }
    }

    let path = local_token_path();
    match fs::read_to_string(&path) {
        Ok(token) => (token.trim().to_string(), TokenSource::LocalFile(path)),
        Err(_) => (String::new(), TokenSource::Missing(path)),
    }
}

fn local_token_path() -> PathBuf {
    if let Ok(path) = env::var("MISTER_AGENT_TOKEN_FILE") {
        let path = path.trim();
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("build/mister-agent.token")
}

pub fn fetch_dashboard(host: &str) -> DashboardSnapshot {
    let mut snapshot = DashboardSnapshot::initial(host);
    let (token, source) = read_token();
    snapshot.token_source = source.label();

    let client = AgentClient::new(host.to_string(), token);
    match client.request("ping", json!({})) {
        Ok(_) => {
            snapshot.connection_state = ConnectionOutcome::Ready.label().to_string();
            snapshot.agent_status = "Authenticated TCP agent responded".to_string();
        }
        Err(AgentError::Unauthorized) => {
            snapshot.connection_state = ConnectionOutcome::Unauthenticated.label().to_string();
            snapshot.agent_status = "Agent rejected the token".to_string();
            snapshot.last_error = "unauthorized".to_string();
            return snapshot;
        }
        Err(AgentError::Unreachable(err)) => {
            snapshot.connection_state = ConnectionOutcome::Unreachable.label().to_string();
            snapshot.agent_status = "No TCP response".to_string();
            snapshot.last_error = err;
            return snapshot;
        }
        Err(err) => {
            snapshot.connection_state = ConnectionOutcome::ProtocolError.label().to_string();
            snapshot.agent_status = "Ping failed".to_string();
            snapshot.last_error = err.to_string();
            return snapshot;
        }
    }

    match client.request("status", json!({})) {
        Ok(status) => apply_agent_status(&mut snapshot, &status),
        Err(err) => snapshot.last_error = err.to_string(),
    }

    match client.request("magik", json!({"action": "status"})) {
        Ok(status) => apply_magik_status(&mut snapshot, &status),
        Err(err) => snapshot.last_error = err.to_string(),
    }

    snapshot
}

pub fn fetch_sd_directory(
    host: &str,
    path: &str,
    show_hidden: bool,
) -> Result<SdDirectoryListing, AgentError> {
    let (token, _) = read_token();
    let client = AgentClient::new(host.to_string(), token);
    let value = client.request(
        "sd_list_dir",
        json!({ "path": path, "show_hidden": show_hidden }),
    )?;
    parse_sd_directory(&value)
}

struct AgentClient {
    host: String,
    token: String,
}

impl AgentClient {
    fn new(host: String, token: String) -> Self {
        Self { host, token }
    }

    fn request(&self, cmd: &str, args: Value) -> Result<Value, AgentError> {
        let addr = format!("{}:{AGENT_PORT}", self.host)
            .to_socket_addrs()
            .map_err(|err| AgentError::Unreachable(err.to_string()))?
            .next()
            .ok_or_else(|| {
                AgentError::Unreachable("could not resolve MiSTer agent host".to_string())
            })?;

        let start = Instant::now();
        let mut stream = TcpStream::connect_timeout(&addr, REQUEST_TIMEOUT)
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;
        stream
            .set_read_timeout(Some(REQUEST_TIMEOUT))
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;
        stream
            .set_write_timeout(Some(REQUEST_TIMEOUT))
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;

        let request = json!({
            "token": self.token,
            "id": 1,
            "cmd": cmd,
            "args": args,
        });
        writeln!(stream, "{request}").map_err(|err| AgentError::Unreachable(err.to_string()))?;
        stream
            .flush()
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;

        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;
        parse_response(&line, start.elapsed())
    }
}

fn parse_response(line: &str, _elapsed: Duration) -> Result<Value, AgentError> {
    if line.trim().is_empty() {
        return Err(AgentError::Protocol(
            "empty response from agent".to_string(),
        ));
    }
    let response: Value = serde_json::from_str(line.trim())
        .map_err(|err| AgentError::Protocol(format!("invalid JSON response: {err}")))?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    } else {
        let error = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("agent command failed");
        if error == "unauthorized" {
            Err(AgentError::Unauthorized)
        } else {
            Err(AgentError::Command(error.to_string()))
        }
    }
}

fn apply_agent_status(snapshot: &mut DashboardSnapshot, status: &Value) {
    snapshot.agent_version = string_at(status, "/agent/version")
        .unwrap_or("-")
        .to_string();
    snapshot.agent_uptime =
        uptime_label(status.pointer("/agent/uptime_ms").and_then(Value::as_u64));
    let ip = string_at(status, "/network/ip").unwrap_or("-");
    let carrier = string_at(status, "/network/carrier").unwrap_or("-");
    let operstate = string_at(status, "/network/operstate").unwrap_or("-");
    snapshot.network_summary = format!("ip {ip}; carrier {carrier}; state {operstate}");
    snapshot.mac_address = string_at(status, "/network/mac").unwrap_or("-").to_string();
    snapshot.main_process = process_summary(status, "MiSTer_MagiK");
    snapshot.launcher_process = process_summary(status, "mister-magik-fb");
}

fn apply_magik_status(snapshot: &mut DashboardSnapshot, status: &Value) {
    snapshot.main_process = process_summary(status, "MiSTer_MagiK");
    snapshot.launcher_process = process_summary(status, "mister-magik-fb");
    snapshot.slint_status_freshness = status
        .pointer("/files/slint_status_current")
        .and_then(Value::as_bool)
        .map(|fresh| if fresh { "current" } else { "stale" }.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let main_status = status.pointer("/files/main_status").unwrap_or(&Value::Null);
    snapshot.visible_owner = string_at(main_status, "/visible_owner")
        .unwrap_or("unknown")
        .to_string();
    snapshot.launcher_state = string_at(main_status, "/launcher_state")
        .or_else(|| string_at(main_status, "/state"))
        .unwrap_or("unknown")
        .to_string();

    let slint_status = status
        .pointer("/files/slint_status")
        .unwrap_or(&Value::Null);
    snapshot.catalog_summary = catalog_summary(slint_status);
    snapshot.screen_summary = screen_summary(slint_status);
    snapshot.input_summary = input_summary(slint_status);
}

fn parse_sd_directory(value: &Value) -> Result<SdDirectoryListing, AgentError> {
    if string_at(value, "/schema") != Some("mister-magik-sd-list-dir-v1") {
        return Err(AgentError::Protocol(
            "unexpected sd_list_dir response schema".to_string(),
        ));
    }
    let path = string_at(value, "/path")
        .ok_or_else(|| AgentError::Protocol("missing sd_list_dir path".to_string()))?
        .to_string();
    let elapsed_ms = value
        .pointer("/elapsed_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let entries = value
        .pointer("/entries")
        .and_then(Value::as_array)
        .ok_or_else(|| AgentError::Protocol("missing sd_list_dir entries".to_string()))?
        .iter()
        .map(parse_sd_entry)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SdDirectoryListing {
        path,
        entries,
        elapsed_ms,
    })
}

fn parse_sd_entry(value: &Value) -> Result<SdEntry, AgentError> {
    let name = string_at(value, "/name")
        .ok_or_else(|| AgentError::Protocol("missing sd entry name".to_string()))?
        .to_string();
    let path = string_at(value, "/path")
        .ok_or_else(|| AgentError::Protocol("missing sd entry path".to_string()))?
        .to_string();
    let kind = match string_at(value, "/kind") {
        Some("directory") => SdEntryKind::Directory,
        Some("file") => SdEntryKind::File,
        Some(other) => {
            return Err(AgentError::Protocol(format!(
                "unsupported sd entry kind: {other}"
            )))
        }
        None => return Err(AgentError::Protocol("missing sd entry kind".to_string())),
    };
    Ok(SdEntry {
        name,
        path,
        kind,
        size: value.pointer("/size").and_then(Value::as_u64).unwrap_or(0),
        modified_unix_ms: value
            .pointer("/modified_unix_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        readonly: value
            .pointer("/readonly")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        hidden: value
            .pointer("/hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(err) => write!(f, "{err}"),
            Self::Unauthorized => write!(f, "unauthorized"),
            Self::Protocol(err) => write!(f, "{err}"),
            Self::Command(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for AgentError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_response_returns_result() {
        let value = parse_response(
            r#"{"id":1,"ok":true,"result":{"pong":true}}"#,
            Duration::ZERO,
        )
        .expect("response should parse");
        assert_eq!(value["pong"], true);
    }

    #[test]
    fn parse_response_returns_null_for_missing_result() {
        let value =
            parse_response(r#"{"id":1,"ok":true}"#, Duration::ZERO).expect("response should parse");

        assert_eq!(value, Value::Null);
    }

    #[test]
    fn parse_response_detects_unauthorized() {
        let err = parse_response(
            r#"{"id":1,"ok":false,"error":"unauthorized"}"#,
            Duration::ZERO,
        )
        .expect_err("response should fail");
        assert!(matches!(err, AgentError::Unauthorized));
    }

    #[test]
    fn parse_response_reports_protocol_and_command_errors() {
        let empty = parse_response("", Duration::ZERO).expect_err("empty response");
        assert!(
            matches!(empty, AgentError::Protocol(message) if message == "empty response from agent")
        );

        let bad_json = parse_response("not json", Duration::ZERO).expect_err("bad json");
        assert!(
            matches!(bad_json, AgentError::Protocol(message) if message.contains("invalid JSON response"))
        );

        let command = parse_response(
            r#"{"id":1,"ok":false,"error":"bad-command"}"#,
            Duration::ZERO,
        )
        .expect_err("command error");
        assert!(matches!(command, AgentError::Command(message) if message == "bad-command"));

        let default_command = parse_response(r#"{"id":1,"ok":false}"#, Duration::ZERO)
            .expect_err("default command error");
        assert!(
            matches!(default_command, AgentError::Command(message) if message == "agent command failed")
        );
    }

    #[test]
    fn token_source_labels_are_human_readable() {
        assert_eq!(TokenSource::Env.label(), "MISTER_AGENT_TOKEN");
        assert_eq!(
            TokenSource::LocalFile(PathBuf::from("/tmp/token")).label(),
            "/tmp/token"
        );
        assert_eq!(
            TokenSource::Missing(PathBuf::from("/tmp/token")).label(),
            "missing (/tmp/token)"
        );
    }

    #[test]
    fn read_token_prefers_env_then_configured_file_then_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        let token_path = env::temp_dir().join(format!("mister-agent-token-{}", std::process::id()));
        fs::write(&token_path, " file-token \n").expect("write token fixture");

        env::set_var("MISTER_AGENT_TOKEN_FILE", &token_path);
        env::remove_var("MISTER_AGENT_TOKEN");
        let (token, source) = read_token();
        assert_eq!(token, "file-token");
        assert_eq!(source, TokenSource::LocalFile(token_path.clone()));

        env::set_var("MISTER_AGENT_TOKEN", " env-token ");
        let (token, source) = read_token();
        assert_eq!(token, "env-token");
        assert_eq!(source, TokenSource::Env);

        env::set_var("MISTER_AGENT_TOKEN", "   ");
        env::set_var(
            "MISTER_AGENT_TOKEN_FILE",
            token_path.with_extension("missing"),
        );
        let (token, source) = read_token();
        assert_eq!(token, "");
        assert!(matches!(source, TokenSource::Missing(_)));

        env::remove_var("MISTER_AGENT_TOKEN");
        env::remove_var("MISTER_AGENT_TOKEN_FILE");
        let _ = fs::remove_file(token_path);
    }

    #[test]
    fn apply_agent_status_formats_network_and_process_fields() {
        let mut snapshot = DashboardSnapshot::initial("host");
        let status = json!({
            "agent": {"version": "1.2.3", "uptime_ms": 125000},
            "network": {
                "ip": "192.168.1.117",
                "carrier": "1",
                "operstate": "up",
                "mac": "02:00:00:00:00:01"
            },
            "processes": {"MiSTer_MagiK": [10, 11], "mister-magik-fb": []}
        });

        apply_agent_status(&mut snapshot, &status);

        assert_eq!(snapshot.agent_version, "1.2.3");
        assert_eq!(snapshot.agent_uptime, "2m 5s");
        assert_eq!(
            snapshot.network_summary,
            "ip 192.168.1.117; carrier 1; state up"
        );
        assert_eq!(snapshot.mac_address, "02:00:00:00:00:01");
        assert_eq!(snapshot.main_process, "2 running (10, 11)");
        assert_eq!(snapshot.launcher_process, "not running");
    }

    #[test]
    fn apply_magik_status_extracts_runtime_fields() {
        let mut snapshot = DashboardSnapshot::initial("host");
        let status = json!({
            "processes": {"MiSTer_MagiK": [10], "mister-magik-fb": [20]},
            "files": {
                "slint_status_current": true,
                "main_status": {"visible_owner": "fb0", "launcher_state": "LauncherActive"},
                "slint_status": {"screen": "Home", "scene": "launcher", "catalog_ready": true, "catalog_games": 5, "catalog_systems": 2, "input_pad_count": 1, "active_pad_name": "Pad"}
            }
        });
        apply_magik_status(&mut snapshot, &status);
        assert_eq!(snapshot.slint_status_freshness, "current");
        assert_eq!(snapshot.visible_owner, "fb0");
        assert_eq!(snapshot.launcher_state, "LauncherActive");
        assert_eq!(snapshot.catalog_summary, "ready; 5 games; 2 systems");
    }

    #[test]
    fn apply_magik_status_uses_fallbacks_for_stale_and_missing_runtime_files() {
        let mut snapshot = DashboardSnapshot::initial("host");
        let status = json!({
            "processes": {"MiSTer_MagiK": [], "mister-magik-fb": [20]},
            "files": {
                "slint_status_current": false,
                "main_status": {"state": "Booting"},
                "slint_status": {"catalog_ready": false, "catalog_scan_message": "scanning"}
            }
        });

        apply_magik_status(&mut snapshot, &status);

        assert_eq!(snapshot.main_process, "not running");
        assert_eq!(snapshot.launcher_process, "1 running (20)");
        assert_eq!(snapshot.slint_status_freshness, "stale");
        assert_eq!(snapshot.visible_owner, "unknown");
        assert_eq!(snapshot.launcher_state, "Booting");
        assert_eq!(
            snapshot.catalog_summary,
            "not ready; - games; - systems; scanning"
        );
        assert_eq!(
            snapshot.screen_summary,
            "unknown / unknown; - fps; last frame -ms ago"
        );
        assert_eq!(snapshot.input_summary, "- pad(s); active: none");
    }

    #[test]
    fn parse_sd_directory_validates_schema_and_entries() {
        let listing = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v1",
            "path": "/",
            "elapsed_ms": 12,
            "entries": [
                {
                    "name": "_Arcade",
                    "path": "/_Arcade",
                    "kind": "directory",
                    "size": 0,
                    "modified_unix_ms": 0,
                    "readonly": false,
                    "hidden": false
                },
                {
                    "name": "MiSTer.ini",
                    "path": "/MiSTer.ini",
                    "kind": "file",
                    "size": 42,
                    "modified_unix_ms": 1234,
                    "readonly": true,
                    "hidden": true
                }
            ]
        }))
        .expect("sd directory response should parse");

        assert_eq!(listing.path, "/");
        assert_eq!(listing.elapsed_ms, 12);
        assert_eq!(listing.entries[0].kind, SdEntryKind::Directory);
        assert_eq!(listing.entries[1].kind, SdEntryKind::File);
        assert_eq!(listing.entries[1].size, 42);
        assert!(listing.entries[1].readonly);
        assert!(listing.entries[1].hidden);

        let err = parse_sd_directory(&json!({"schema": "wrong"}))
            .expect_err("schema mismatch should fail");
        assert!(matches!(err, AgentError::Protocol(message) if message.contains("schema")));
    }

    #[test]
    fn parse_sd_directory_reports_missing_container_fields() {
        let missing_path = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v1",
            "entries": []
        }))
        .expect_err("missing path should fail");
        assert!(
            matches!(missing_path, AgentError::Protocol(message) if message == "missing sd_list_dir path")
        );

        let missing_entries = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v1",
            "path": "/"
        }))
        .expect_err("missing entries should fail");
        assert!(
            matches!(missing_entries, AgentError::Protocol(message) if message == "missing sd_list_dir entries")
        );
    }

    #[test]
    fn parse_sd_directory_reports_entry_shape_errors() {
        let missing_name = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v1",
            "path": "/",
            "entries": [{"path": "/bad", "kind": "file"}]
        }))
        .expect_err("missing entry name should fail");
        assert!(
            matches!(missing_name, AgentError::Protocol(message) if message == "missing sd entry name")
        );

        let missing_path = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v1",
            "path": "/",
            "entries": [{"name": "bad", "kind": "file"}]
        }))
        .expect_err("missing entry path should fail");
        assert!(
            matches!(missing_path, AgentError::Protocol(message) if message == "missing sd entry path")
        );

        let missing_kind = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v1",
            "path": "/",
            "entries": [{"name": "bad", "path": "/bad"}]
        }))
        .expect_err("missing entry kind should fail");
        assert!(
            matches!(missing_kind, AgentError::Protocol(message) if message == "missing sd entry kind")
        );

        let unsupported_kind = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v1",
            "path": "/",
            "entries": [{"name": "bad", "path": "/bad", "kind": "symlink"}]
        }))
        .expect_err("unsupported entry kind should fail");
        assert!(
            matches!(unsupported_kind, AgentError::Protocol(message) if message == "unsupported sd entry kind: symlink")
        );
    }

    #[test]
    fn agent_error_display_matches_user_facing_message() {
        assert_eq!(
            AgentError::Unreachable("network down".to_string()).to_string(),
            "network down"
        );
        assert_eq!(AgentError::Unauthorized.to_string(), "unauthorized");
        assert_eq!(
            AgentError::Protocol("bad json".to_string()).to_string(),
            "bad json"
        );
        assert_eq!(
            AgentError::Command("bad command".to_string()).to_string(),
            "bad command"
        );
    }
}
