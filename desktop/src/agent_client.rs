use crate::app_state::{
    catalog_summary, input_summary, process_summary, screen_summary, string_at, uptime_label,
    ConnectionOutcome, DashboardSnapshot,
};
use crate::sd_card::{SdDirectoryListing, SdEntry, SdEntryKind};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FramebufferCapture {
    pub png_path: PathBuf,
    pub rgba_pixels: Vec<u8>,
    pub width: u64,
    pub height: u64,
    pub bpp: u64,
    pub raw_bytes: u64,
    pub payload_bytes: u64,
    pub encoding: String,
    pub png_bytes: u64,
    pub png_hex_bytes: u64,
    pub timing: FramebufferCaptureTiming,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FramebufferCaptureTiming {
    pub request_received_uptime_ms: u64,
    pub dispatch_us: u64,
    pub geometry_us: u64,
    pub raw_read_us: u64,
    pub rgba_convert_us: u64,
    pub zlib_encode_us: u64,
    pub png_wrap_us: u64,
    pub png_total_us: u64,
    pub hex_encode_us: u64,
    pub lz4_encode_us: u64,
    pub total_us: u64,
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

pub fn fetch_framebuffer_capture(host: &str) -> Result<FramebufferCapture, AgentError> {
    let (token, _) = read_token();
    let client = AgentClient::new(host.to_string(), token);
    let (value, payload) = client.request_binary("framebuffer_capture_lz4_stream", json!({}))?;
    parse_framebuffer_capture_lz4(&value, &payload)
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

    fn request_binary(&self, cmd: &str, args: Value) -> Result<(Value, Vec<u8>), AgentError> {
        let addr = format!("{}:{AGENT_PORT}", self.host)
            .to_socket_addrs()
            .map_err(|err| AgentError::Unreachable(err.to_string()))?
            .next()
            .ok_or_else(|| {
                AgentError::Unreachable("could not resolve MiSTer agent host".to_string())
            })?;

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

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;
        let value = parse_response(&line, Duration::ZERO)?;
        let payload_bytes = value
            .pointer("/payload_bytes")
            .or_else(|| value.pointer("/raw_bytes"))
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                AgentError::Protocol("binary response missing payload byte count".to_string())
            })? as usize;
        let mut payload = vec![0u8; payload_bytes];
        reader
            .read_exact(&mut payload)
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;
        Ok((value, payload))
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

#[cfg(test)]
fn parse_framebuffer_capture(value: &Value) -> Result<FramebufferCapture, AgentError> {
    if string_at(value, "/schema") != Some("mister-magik-framebuffer-capture-v1") {
        return Err(AgentError::Protocol(
            "unexpected framebuffer_capture response schema".to_string(),
        ));
    }
    let png_hex = string_at(value, "/png_hex")
        .ok_or_else(|| AgentError::Protocol("missing framebuffer png_hex".to_string()))?;
    let png = decode_hex(png_hex).map_err(AgentError::Protocol)?;
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(AgentError::Protocol(
            "framebuffer capture did not return a PNG".to_string(),
        ));
    }
    let png_path = local_framebuffer_capture_path();
    fs::write(&png_path, &png)
        .map_err(|err| AgentError::Unreachable(format!("write framebuffer PNG: {err}")))?;
    Ok(FramebufferCapture {
        png_path,
        rgba_pixels: Vec::new(),
        width: value.pointer("/width").and_then(Value::as_u64).unwrap_or(0),
        height: value
            .pointer("/height")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        bpp: value.pointer("/bpp").and_then(Value::as_u64).unwrap_or(0),
        raw_bytes: value
            .pointer("/raw_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        payload_bytes: value
            .pointer("/png_hex_bytes")
            .and_then(Value::as_u64)
            .unwrap_or((png.len() * 2) as u64),
        encoding: "png-hex".to_string(),
        png_bytes: png.len() as u64,
        png_hex_bytes: value
            .pointer("/png_hex_bytes")
            .and_then(Value::as_u64)
            .unwrap_or((png.len() * 2) as u64),
        timing: parse_framebuffer_capture_timing(value),
    })
}

fn parse_framebuffer_capture_lz4(
    value: &Value,
    payload: &[u8],
) -> Result<FramebufferCapture, AgentError> {
    if string_at(value, "/schema") != Some("mister-magik-framebuffer-raw-stream-v1") {
        return Err(AgentError::Protocol(
            "unexpected framebuffer raw stream response schema".to_string(),
        ));
    }
    let encoding = string_at(value, "/encoding").unwrap_or("raw");
    let raw = if encoding == "lz4-block-size-prepended" {
        lz4_flex::decompress_size_prepended(payload)
            .map_err(|err| AgentError::Protocol(format!("decompress framebuffer LZ4: {err}")))?
    } else {
        payload.to_vec()
    };
    let width = value.pointer("/width").and_then(Value::as_u64).unwrap_or(0);
    let height = value.pointer("/height").and_then(Value::as_u64).unwrap_or(0);
    let stride = value.pointer("/stride").and_then(Value::as_u64).unwrap_or(0);
    let bpp = value.pointer("/bpp").and_then(Value::as_u64).unwrap_or(0);
    let expected_raw = value
        .pointer("/raw_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(raw.len() as u64) as usize;
    if raw.len() != expected_raw {
        return Err(AgentError::Protocol(format!(
            "decoded framebuffer size mismatch expected={expected_raw} actual={}",
            raw.len()
        )));
    }
    let rgba_pixels = framebuffer_raw_to_rgba(&raw, width, height, stride, bpp)?;
    Ok(FramebufferCapture {
        png_path: PathBuf::new(),
        rgba_pixels,
        width,
        height,
        bpp,
        raw_bytes: raw.len() as u64,
        payload_bytes: payload.len() as u64,
        encoding: encoding.to_string(),
        png_bytes: 0,
        png_hex_bytes: 0,
        timing: parse_framebuffer_capture_timing(value),
    })
}

fn framebuffer_raw_to_rgba(
    raw: &[u8],
    width: u64,
    height: u64,
    stride: u64,
    bpp: u64,
) -> Result<Vec<u8>, AgentError> {
    let width = usize::try_from(width)
        .map_err(|_| AgentError::Protocol("framebuffer width too large".to_string()))?;
    let height = usize::try_from(height)
        .map_err(|_| AgentError::Protocol("framebuffer height too large".to_string()))?;
    let stride = usize::try_from(stride)
        .map_err(|_| AgentError::Protocol("framebuffer stride too large".to_string()))?;
    let bytes_per_pixel = match bpp {
        16 => 2,
        32 => 4,
        _ => {
            return Err(AgentError::Protocol(format!(
                "unsupported framebuffer bpp: {bpp}"
            )))
        }
    };
    let packed_stride = width
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| AgentError::Protocol("framebuffer row size overflow".to_string()))?;
    if stride < packed_stride {
        return Err(AgentError::Protocol(format!(
            "framebuffer stride {stride} smaller than packed row {packed_stride}"
        )));
    }
    let expected = stride
        .checked_mul(height)
        .ok_or_else(|| AgentError::Protocol("framebuffer byte size overflow".to_string()))?;
    if raw.len() < expected {
        return Err(AgentError::Protocol(format!(
            "framebuffer raw too short expected={expected} actual={}",
            raw.len()
        )));
    }

    let mut rgba = Vec::with_capacity(
        width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| AgentError::Protocol("RGBA image size overflow".to_string()))?,
    );
    for y in 0..height {
        for x in 0..width {
            match bpp {
                16 => {
                    let i = y * stride + x * 2;
                    let v = u16::from_le_bytes([raw[i], raw[i + 1]]);
                    let r5 = (v >> 11) & 0x1f;
                    let g6 = (v >> 5) & 0x3f;
                    let b5 = v & 0x1f;
                    rgba.extend_from_slice(&[
                        ((r5 << 3) | (r5 >> 2)) as u8,
                        ((g6 << 2) | (g6 >> 4)) as u8,
                        ((b5 << 3) | (b5 >> 2)) as u8,
                        0xff,
                    ]);
                }
                32 => {
                    let i = y * stride + x * 4;
                    rgba.extend_from_slice(&[raw[i + 2], raw[i + 1], raw[i], 0xff]);
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(rgba)
}

fn parse_framebuffer_capture_timing(value: &Value) -> FramebufferCaptureTiming {
    fn field(value: &Value, name: &str) -> u64 {
        value
            .pointer(&format!("/timings/{name}"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }

    FramebufferCaptureTiming {
        request_received_uptime_ms: field(value, "request_received_uptime_ms"),
        dispatch_us: field(value, "dispatch_us"),
        geometry_us: field(value, "geometry_us"),
        raw_read_us: field(value, "raw_read_us"),
        rgba_convert_us: field(value, "rgba_convert_us"),
        zlib_encode_us: field(value, "zlib_encode_us"),
        png_wrap_us: field(value, "png_wrap_us"),
        png_total_us: field(value, "png_total_us"),
        hex_encode_us: field(value, "hex_encode_us"),
        lz4_encode_us: field(value, "lz4_encode_us"),
        total_us: field(value, "total_us"),
    }
}

#[cfg(test)]
fn local_framebuffer_capture_path() -> PathBuf {
    env::temp_dir().join(format!(
        "mister-magik-framebuffer-{}.png",
        std::process::id()
    ))
}

#[cfg(test)]
fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err("hex payload has odd length".to_string());
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let raw = hex.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        let hi = hex_value(raw[i])?;
        let lo = hex_value(raw[i + 1])?;
        bytes.push((hi << 4) | lo);
        i += 2;
    }
    Ok(bytes)
}

#[cfg(test)]
fn hex_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(format!("invalid hex byte: {byte}")),
    }
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
    fn parse_framebuffer_capture_writes_png_file() {
        let capture = parse_framebuffer_capture(&json!({
            "schema": "mister-magik-framebuffer-capture-v1",
            "width": 2,
            "height": 1,
            "bpp": 16,
            "raw_bytes": 4,
            "png_hex_bytes": 24,
            "timings": {
                "raw_read_us": 10,
                "zlib_encode_us": 20,
                "total_us": 30
            },
            "png_hex": "89504e470d0a1a0a00000000"
        }))
        .expect("framebuffer capture should parse");

        assert_eq!(capture.width, 2);
        assert_eq!(capture.height, 1);
        assert_eq!(capture.bpp, 16);
        assert_eq!(capture.raw_bytes, 4);
        assert_eq!(capture.payload_bytes, 24);
        assert_eq!(capture.encoding, "png-hex");
        assert_eq!(capture.png_bytes, 12);
        assert_eq!(capture.png_hex_bytes, 24);
        assert_eq!(capture.timing.raw_read_us, 10);
        assert_eq!(capture.timing.zlib_encode_us, 20);
        assert_eq!(capture.timing.total_us, 30);
        assert!(fs::read(&capture.png_path)
            .expect("capture PNG should be written")
            .starts_with(b"\x89PNG\r\n\x1a\n"));
        let _ = fs::remove_file(capture.png_path);
    }

    #[test]
    fn parse_framebuffer_lz4_capture_expands_rgb565_pixels() {
        let raw = [0x00, 0xf8, 0xe0, 0x07];
        let payload = lz4_flex::compress_prepend_size(&raw);
        let capture = parse_framebuffer_capture_lz4(
            &json!({
                "schema": "mister-magik-framebuffer-raw-stream-v1",
                "width": 2,
                "height": 1,
                "stride": 4,
                "bpp": 16,
                "format": "rgb565-le",
                "encoding": "lz4-block-size-prepended",
                "raw_bytes": 4,
                "payload_bytes": payload.len(),
                "timings": {
                    "raw_read_us": 10,
                    "lz4_encode_us": 20,
                    "total_us": 30
                }
            }),
            &payload,
        )
        .expect("LZ4 framebuffer capture should parse");

        assert_eq!(capture.width, 2);
        assert_eq!(capture.height, 1);
        assert_eq!(capture.bpp, 16);
        assert_eq!(capture.raw_bytes, 4);
        assert_eq!(capture.payload_bytes, payload.len() as u64);
        assert_eq!(capture.encoding, "lz4-block-size-prepended");
        assert_eq!(
            capture.rgba_pixels,
            vec![255, 0, 0, 255, 0, 255, 0, 255]
        );
        assert_eq!(capture.timing.raw_read_us, 10);
        assert_eq!(capture.timing.lz4_encode_us, 20);
        assert_eq!(capture.timing.total_us, 30);
    }

    #[test]
    fn parse_framebuffer_capture_rejects_bad_shape() {
        let schema = parse_framebuffer_capture(&json!({"schema": "wrong"}))
            .expect_err("schema mismatch should fail");
        assert!(matches!(schema, AgentError::Protocol(message) if message.contains("schema")));

        let not_png = parse_framebuffer_capture(&json!({
            "schema": "mister-magik-framebuffer-capture-v1",
            "png_hex": "00010203"
        }))
        .expect_err("non-PNG payload should fail");
        assert!(
            matches!(not_png, AgentError::Protocol(message) if message.contains("did not return a PNG"))
        );
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
