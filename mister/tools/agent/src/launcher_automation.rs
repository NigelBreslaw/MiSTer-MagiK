// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Closed, authenticated bridge to the launcher's volatile UI automation socket.

use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SESSION_SCHEMA: &str = "mister-magik-ui-automation-session-v1";
const REQUEST_SCHEMA: &str = "mister-magik-ui-automation-request-v1";
const DESCRIPTOR_PATH: &str = "/tmp/mister-magik/ui-automation-session.json";
const SOCKET_PATH: &str = "/tmp/mister-magik/ui-automation.sock";
const FAILURE_PATH: &str = "/tmp/mister-magik/ui-automation-failure.json";
const STATUS_PATH: &str = "/tmp/mister-magik/status.json";
const MAIN_STATUS_PATH: &str = "/tmp/mister-magik/main-status.json";
const MAX_SESSION_SECONDS: u64 = 120;
static CLIENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn begin(args: Value) -> Result<Value, String> {
    let _ = fs::remove_file(FAILURE_PATH);
    let expected_build_version = required_text(&args, "expected_build_version")?;
    let expected_source_revision = required_text(&args, "expected_source_revision")?;
    let expected_main_generation = required_u64(&args, "expected_main_generation")?;
    let lifetime_seconds = required_u64(&args, "lifetime_seconds")?;
    if lifetime_seconds == 0 || lifetime_seconds > MAX_SESSION_SECONDS {
        return Err("automation lifetime must be in 1..=120 seconds".to_string());
    }
    let status = read_json(STATUS_PATH)?;
    let launcher_pid = status
        .get("pid")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| "launcher status has no valid pid".to_string())?;
    require_equal_text(&status, "/build/version", expected_build_version)?;
    require_equal_text(&status, "/build/source_revision", expected_source_revision)?;
    let main_status = read_json(MAIN_STATUS_PATH)?;
    let main_generation = main_status
        .get("main_generation")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Main status has no generation".to_string())?;
    if main_generation != expected_main_generation {
        return Err(format!(
            "stale Main generation expected={expected_main_generation} actual={main_generation}"
        ));
    }
    let nonce = random_nonce()?;
    let created_unix_ms = unix_ms();
    let expires_unix_ms = created_unix_ms.saturating_add(lifetime_seconds.saturating_mul(1_000));
    let descriptor = json!({
        "schema": SESSION_SCHEMA,
        "nonce": nonce,
        "expected_build_version": expected_build_version,
        "expected_source_revision": expected_source_revision,
        "launcher_pid": launcher_pid,
        "main_generation": main_generation,
        "created_unix_ms": created_unix_ms,
        "expires_unix_ms": expires_unix_ms,
    });
    write_descriptor(&descriptor)?;
    wait_for_launcher_socket()?;
    Ok(json!({
        "schema": SESSION_SCHEMA,
        "nonce": descriptor["nonce"],
        "launcher_pid": launcher_pid,
        "main_generation": main_generation,
        "expires_unix_ms": expires_unix_ms,
    }))
}

fn wait_for_launcher_socket() -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if Path::new(SOCKET_PATH).exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = fs::remove_file(DESCRIPTOR_PATH);
    Err("launcher did not open the automation socket".to_string())
}

pub(crate) fn request(args: Value) -> Result<Value, String> {
    let nonce = required_text(&args, "nonce")?;
    validate_nonce(nonce)?;
    let command = validated_command(&args)?;
    let request = json!({
        "schema": REQUEST_SCHEMA,
        "nonce": nonce,
        "command": command,
    });
    request_socket(&request)
}

fn validated_command(args: &Value) -> Result<Value, String> {
    let kind = required_text(args, "kind")?;
    match kind {
        "snapshot" | "release_all" | "end" => Ok(json!({"kind":kind})),
        "tap" => {
            let button = required_text(args, "button")?;
            validate_button(button)?;
            Ok(json!({"kind":kind,"button":button}))
        }
        "hold" => {
            let button = required_text(args, "button")?;
            validate_button(button)?;
            let duration_ms = required_u64(args, "duration_ms")?;
            if duration_ms == 0
                || duration_ms > mister_magik_agent_protocol::LAUNCHER_AUTOMATION_MAX_HOLD_MS
            {
                return Err(format!(
                    "automation hold must be in 1..={} milliseconds",
                    mister_magik_agent_protocol::LAUNCHER_AUTOMATION_MAX_HOLD_MS
                ));
            }
            Ok(json!({"kind":kind,"button":button,"duration_ms":duration_ms}))
        }
        _ => Err("unsupported automation command".to_string()),
    }
}

fn validate_button(button: &str) -> Result<(), String> {
    if matches!(
        button,
        "up" | "down" | "left" | "right" | "a" | "b" | "home" | "x" | "y"
    ) {
        Ok(())
    } else {
        Err("unsupported automation button".to_string())
    }
}

fn request_socket(request: &Value) -> Result<Value, String> {
    let client_path = client_socket_path();
    let cleanup = SocketCleanup(client_path.clone());
    let _ = fs::remove_file(&client_path);
    let socket = UnixDatagram::bind(&client_path)
        .map_err(|error| format!("bind automation client socket: {error}"))?;
    fs::set_permissions(&client_path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("protect automation client socket: {error}"))?;
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("set automation response timeout: {error}"))?;
    socket
        .send_to(request.to_string().as_bytes(), SOCKET_PATH)
        .map_err(|error| automation_socket_error("send automation request", error))?;
    let mut bytes = [0_u8; 64 * 1024];
    let length = socket
        .recv(&mut bytes)
        .map_err(|error| format!("receive automation response: {error}"))?;
    drop(cleanup);
    let response: Value = serde_json::from_slice(&bytes[..length])
        .map_err(|error| format!("decode automation response: {error}"))?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("automation command failed")
            .to_string());
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

fn automation_socket_error(operation: &str, error: std::io::Error) -> String {
    let retained = fs::read_to_string(FAILURE_PATH)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|failure| failure.to_string());
    match retained {
        Some(retained) => format!("{operation}: {error}; retained_failure={retained}"),
        None => format!("{operation}: {error}"),
    }
}

struct SocketCleanup(PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn write_descriptor(value: &Value) -> Result<(), String> {
    let path = Path::new(DESCRIPTOR_PATH);
    let parent = path
        .parent()
        .ok_or_else(|| "automation descriptor has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create automation runtime directory: {error}"))?;
    let temporary = parent.join(format!(".ui-automation-session-{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| format!("create automation descriptor: {error}"))?;
    let result = (|| {
        serde_json::to_writer(&mut file, value)
            .map_err(|error| format!("encode automation descriptor: {error}"))?;
        file.write_all(b"\n")
            .map_err(|error| format!("finish automation descriptor: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync automation descriptor: {error}"))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("publish automation descriptor: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn random_nonce() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| format!("read automation nonce: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn validate_nonce(value: &str) -> Result<(), String> {
    if (32..=128).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("invalid automation nonce".to_string())
    }
}

fn read_json(path: &str) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|error| format!("read {path}: {error}"))?)
        .map_err(|error| format!("decode {path}: {error}"))
}

fn require_equal_text(value: &Value, pointer: &str, expected: &str) -> Result<(), String> {
    let actual = value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("status is missing {pointer}"))?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "launcher identity mismatch at {pointer} expected={expected} actual={actual}"
        ))
    }
}

fn required_text<'a>(args: &'a Value, field: &str) -> Result<&'a str, String> {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("automation request requires {field}"))
}

fn required_u64(args: &Value, field: &str) -> Result<u64, String> {
    args.get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("automation request requires {field}"))
}

fn client_socket_path() -> PathBuf {
    let sequence = CLIENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!(
        "/tmp/mister-magik/ui-automation-client-{}-{sequence}.sock",
        std::process::id()
    ))
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_contract_is_closed_and_bounded() {
        assert_eq!(
            validated_command(&json!({"kind":"tap","button":"home"})).unwrap(),
            json!({"kind":"tap","button":"home"})
        );
        assert!(validated_command(&json!({"kind":"tap","button":"start"})).is_err());
        assert!(
            validated_command(&json!({
                "kind":"hold",
                "button":"down",
                "duration_ms":mister_magik_agent_protocol::LAUNCHER_AUTOMATION_MAX_HOLD_MS
            }))
            .is_ok()
        );
        assert!(
            validated_command(&json!({
                "kind":"hold",
                "button":"down",
                "duration_ms":mister_magik_agent_protocol::LAUNCHER_AUTOMATION_MAX_HOLD_MS + 1
            }))
            .is_err()
        );
        assert!(validated_command(&json!({"kind":"shell","button":"a"})).is_err());
    }

    #[test]
    fn nonces_are_hex_and_bounded() {
        assert!(validate_nonce(&"a1".repeat(16)).is_ok());
        assert!(validate_nonce("short").is_err());
        assert!(validate_nonce(&"z".repeat(32)).is_err());
    }
}
