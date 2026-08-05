// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::app_state::{
    ConnectionOutcome, DashboardSnapshot, catalog_summary, input_summary, process_summary,
    screen_summary, string_at, uptime_label,
};
use crate::sd_card::{
    SdDirectoryListing, SdEntry, SdEntryKind, SdItemDetail, SdMetadataRow, item_name,
};
use mister_magik_agent_protocol::{self as agent_protocol, ResponseEnvelope};
use mister_magik_framebuffer_stream::{
    FLAG_LZ4_SIZE_PREPENDED, FrameGeometry, FrameHeader, FrameKind, FrameRect,
    MAX_FRAME_SURFACE_BYTES, read_frame,
};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const AGENT_PORT: u16 = agent_protocol::PORT;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const BINARY_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_AGENT_BINARY_PAYLOAD_BYTES: u64 = agent_protocol::MAX_BINARY_PAYLOAD_BYTES;
const SD_DIRECTORY_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
static SD_LIST_PROTOCOL: SdListProtocolCache = SdListProtocolCache::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SdListProtocol {
    Unknown = 0,
    V2 = 1,
    V1 = 2,
}

struct SdListProtocolCache {
    state: AtomicU8,
}

impl SdListProtocolCache {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(SdListProtocol::Unknown as u8),
        }
    }

    fn load(&self) -> SdListProtocol {
        match self.state.load(Ordering::Acquire) {
            value if value == SdListProtocol::V2 as u8 => SdListProtocol::V2,
            value if value == SdListProtocol::V1 as u8 => SdListProtocol::V1,
            _ => SdListProtocol::Unknown,
        }
    }

    fn remember(&self, protocol: SdListProtocol) {
        self.state.store(protocol as u8, Ordering::Release);
    }
}

fn is_unknown_command(error: &AgentError) -> bool {
    matches!(error, AgentError::Command(message) if message == "unknown cmd")
}

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
    pub raw_pixels: Vec<u8>,
    pub raw_stride_bytes: u64,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FramebufferStreamFrame {
    pub capture: FramebufferCapture,
    pub kind: FrameKind,
    pub sequence: u64,
    pub timestamp_us: u64,
    pub geometry: FrameGeometry,
    pub rect: FrameRect,
    pub raw_bytes: u64,
    pub payload_bytes: u64,
    pub timing: FramebufferStreamTiming,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferStreamTiming {
    pub read_started: Instant,
    pub read_complete: Instant,
    pub decompress_complete: Instant,
    pub rgba_complete: Instant,
}

#[derive(Debug)]
pub struct FramebufferStreamDrainStats {
    pub latencies: Vec<Duration>,
    pub payload_bytes: u64,
    pub raw_bytes: u64,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryDatabaseSnapshot {
    pub remote_path: String,
    pub bytes: Vec<u8>,
    pub raw_bytes: u64,
    pub payload_bytes: u64,
    pub checksum: String,
    pub mtime_unix_ms: u64,
}

pub struct FramebufferStream {
    reader: BufReader<TcpStream>,
    control: FramebufferStreamControl,
    state: FramebufferStreamState,
}

pub struct DeviceTelemetryStream {
    reader: BufReader<TcpStream>,
    control: DeviceTelemetryStreamControl,
}

pub struct FramebufferStreamControl {
    stream: TcpStream,
}

pub struct DeviceTelemetryStreamControl {
    stream: TcpStream,
}

struct FramebufferStreamState {
    rgb565: Vec<u8>,
    geometry: Option<FrameGeometry>,
    expected_sequence: Option<u64>,
    awaiting_keyframe: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceTelemetrySample {
    pub seq: u64,
    pub combined_cpu_pct: f64,
    pub presentation: PresentationTelemetrySample,
    pub cores: Vec<CpuCoreTelemetry>,
    pub memory: MemoryTelemetry,
    pub frame_budget: FrameBudgetTelemetry,
    pub launcher: LauncherTelemetry,
    pub magik: ProcessTelemetry,
    pub main: ProcessTelemetry,
    pub network: NetworkTelemetry,
    pub storage: StorageTelemetry,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresentationTelemetrySample {
    pub available: bool,
    pub captured_monotonic_us: u64,
    pub owned_vblank_count: Option<u32>,
    pub presented_vblank_count: Option<u32>,
    pub repeated_vblank_count: Option<u32>,
    pub ownership_loss_count: Option<u32>,
    pub active_sequence: Option<u16>,
    pub magik_ownership: bool,
    pub pending: bool,
    pub lifetime_invariant_valid: bool,
    pub error: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuCoreTelemetry {
    pub label: String,
    pub busy_pct: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryTelemetry {
    pub total_kb: u64,
    pub magik_kb: u64,
    pub main_kb: u64,
    pub other_used_kb: u64,
    pub available_kb: u64,
    pub magik_pct: f64,
    pub other_used_pct: f64,
    pub available_pct: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameBudgetTelemetry {
    pub budget_us: u64,
    pub frames_total: u64,
    pub window_frames: u64,
    pub window_over_budget: u64,
    pub window_over_20ms: u64,
    pub window_over_33ms: u64,
    pub window_max_wall_us: u64,
    pub max_wall_us: u64,
    pub max_vsync_miss_streak: u64,
    pub window_prepare_us: u64,
    pub window_render_us: u64,
    pub window_custom_draw_us: u64,
    pub window_vsync_us: u64,
    pub window_present_us: u64,
    pub recent_frames: Vec<FrameBudgetFrameTelemetry>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameBudgetFrameTelemetry {
    pub frame: u64,
    pub wall_us: u64,
    pub prepare_us: u64,
    pub render_us: u64,
    pub custom_draw_us: u64,
    pub vsync_us: u64,
    pub present_us: u64,
    pub cpu_prepare_us: u64,
    pub cpu_render_us: u64,
    pub cpu_custom_draw_us: u64,
    pub cpu_vsync_us: u64,
    pub cpu_present_us: u64,
    pub process_cpu_us: u64,
    pub vsync_source: String,
    pub vsync_miss_streak: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LauncherTelemetry {
    pub status_current: bool,
    pub idle: bool,
    pub fps: String,
    pub preview_cache_state: String,
    pub ui_thread_cpu: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessTelemetry {
    pub pids: Vec<u64>,
    pub rss_kb: u64,
    pub threads: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkTelemetry {
    pub rx_bytes_per_sec: u64,
    pub tx_bytes_per_sec: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StorageTelemetry {
    pub available_bytes: u64,
    pub total_bytes: u64,
    pub available_pct: f64,
    pub device: String,
    pub activity_valid: bool,
    pub read_bytes_per_sec: u64,
    pub write_bytes_per_sec: u64,
    pub read_pct: f64,
    pub write_pct: f64,
}

impl Default for FramebufferStreamState {
    fn default() -> Self {
        Self {
            rgb565: Vec::new(),
            geometry: None,
            expected_sequence: None,
            awaiting_keyframe: true,
        }
    }
}

pub fn read_token() -> (String, TokenSource) {
    let token = env::var("MISTER_AGENT_TOKEN").ok();
    let token_file = env::var("MISTER_AGENT_TOKEN_FILE").ok();
    read_token_from(token.as_deref(), token_file.as_deref())
}

fn read_token_from(token: Option<&str>, token_file: Option<&str>) -> (String, TokenSource) {
    if let Some(token) = token {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return (token, TokenSource::Env);
        }
    }

    let path = local_token_path_from(token_file);
    match fs::read_to_string(&path) {
        Ok(token) => (token.trim().to_string(), TokenSource::LocalFile(path)),
        Err(_) => (String::new(), TokenSource::Missing(path)),
    }
}

fn local_token_path_from(configured: Option<&str>) -> PathBuf {
    if let Some(path) = configured.map(str::trim).filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
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
    let args = json!({ "path": path, "show_hidden": show_hidden });
    let started = Instant::now();
    let value = match SD_LIST_PROTOCOL.load() {
        SdListProtocol::V1 => {
            client.request_with_read_timeout("sd_list_dir", args, SD_DIRECTORY_REQUEST_TIMEOUT)?
        }
        SdListProtocol::Unknown | SdListProtocol::V2 => {
            match client.request_with_read_timeout(
                "sd_list_dir_v2",
                args.clone(),
                SD_DIRECTORY_REQUEST_TIMEOUT,
            ) {
                Ok(value) => {
                    SD_LIST_PROTOCOL.remember(SdListProtocol::V2);
                    value
                }
                Err(err) if is_unknown_command(&err) => {
                    SD_LIST_PROTOCOL.remember(SdListProtocol::V1);
                    client.request_with_read_timeout(
                        "sd_list_dir",
                        args,
                        SD_DIRECTORY_REQUEST_TIMEOUT,
                    )?
                }
                Err(err) => return Err(err),
            }
        }
    };
    let mut listing = parse_sd_directory(&value)?;
    listing.round_trip_ms = started.elapsed().as_millis() as u64;
    Ok(listing)
}

pub fn fetch_sd_item_detail(host: &str, path: &str) -> Result<SdItemDetail, AgentError> {
    let (token, _) = read_token();
    let client = AgentClient::new(host.to_string(), token);
    let stat = client.request("sd_stat_item_v1", json!({ "path": path }))?;
    let mut detail = parse_sd_item_detail(&stat)?;

    if detail.has_image {
        match client.request_binary("sd_read_preview_image_v1", json!({ "path": path })) {
            Ok((image_meta, payload)) => {
                apply_sd_preview_image(&mut detail, &image_meta, &payload)?
            }
            Err(err) => detail.overview_rows.push(metadata_row(
                "Preview warning",
                &err.to_string(),
                "warning",
            )),
        }
    }

    if detail.is_mra {
        match client.request("sd_parse_mra_v1", json!({ "path": path })) {
            Ok(mra) => apply_sd_mra_detail(&mut detail, &mra)?,
            Err(err) => detail.mra_warnings.push(metadata_row(
                "MRA parse warning",
                &err.to_string(),
                "warning",
            )),
        }
    }

    Ok(detail)
}

pub fn fetch_framebuffer_capture(host: &str) -> Result<FramebufferCapture, AgentError> {
    let (token, _) = read_token();
    let client = AgentClient::new(host.to_string(), token);
    let (value, payload) = client.request_binary("framebuffer_capture_lz4_stream", json!({}))?;
    parse_framebuffer_capture_lz4(&value, &payload)
}

pub fn fetch_library_database_snapshot(host: &str) -> Result<LibraryDatabaseSnapshot, AgentError> {
    let (token, _) = read_token();
    let client = AgentClient::new(host.to_string(), token);
    let (value, payload) =
        client.request_binary("library_database_snapshot_lz4_stream", json!({}))?;
    parse_library_database_snapshot_lz4(&value, &payload)
}

pub fn connect_framebuffer_stream(host: &str) -> Result<FramebufferStream, AgentError> {
    connect_framebuffer_stream_seeded(host, None)
}

pub fn connect_device_telemetry_stream(host: &str) -> Result<DeviceTelemetryStream, AgentError> {
    let (token, _) = read_token();
    let client = AgentClient::new(host.to_string(), token);
    let (_, reader) = client.request_stream(
        "device_telemetry_stream_v2",
        json!({"analytics_mode": "process"}),
    )?;
    let control = DeviceTelemetryStreamControl {
        stream: reader
            .get_ref()
            .try_clone()
            .map_err(|err| AgentError::Unreachable(err.to_string()))?,
    };
    Ok(DeviceTelemetryStream { reader, control })
}

pub fn connect_framebuffer_stream_seeded(
    host: &str,
    seed: Option<&FramebufferCapture>,
) -> Result<FramebufferStream, AgentError> {
    let (token, _) = read_token();
    let client = AgentClient::new(host.to_string(), token);
    let (_, reader) = request_framebuffer_stream_with_retry(&client)?;
    let control = FramebufferStreamControl {
        stream: reader
            .get_ref()
            .try_clone()
            .map_err(|err| AgentError::Unreachable(err.to_string()))?,
    };
    let mut state = FramebufferStreamState::default();
    if let Some(seed) = seed {
        state.seed_from_capture(seed)?;
    }
    Ok(FramebufferStream {
        reader,
        control,
        state,
    })
}

pub fn drain_framebuffer_stream(
    host: &str,
    frames: u64,
) -> Result<FramebufferStreamDrainStats, AgentError> {
    drain_framebuffer_stream_until(host, |count, _elapsed| count >= frames)
}

pub fn drain_framebuffer_stream_for(
    host: &str,
    duration: Duration,
) -> Result<FramebufferStreamDrainStats, AgentError> {
    drain_framebuffer_stream_until(host, |_count, elapsed| elapsed >= duration)
}

fn drain_framebuffer_stream_until(
    host: &str,
    mut done: impl FnMut(u64, Duration) -> bool,
) -> Result<FramebufferStreamDrainStats, AgentError> {
    let (token, _) = read_token();
    let client = AgentClient::new(host.to_string(), token);
    let (_, mut reader) = request_framebuffer_stream_with_retry(&client)?;
    let mut latencies = Vec::new();
    let mut payload_bytes = 0_u64;
    let mut raw_bytes = 0_u64;
    let started = Instant::now();
    while !done(latencies.len() as u64, started.elapsed()) {
        let frame_started = Instant::now();
        let (header, payload) = read_frame(&mut reader)
            .map_err(|err| AgentError::Unreachable(format!("read framebuffer stream: {err}")))?;
        match header.kind {
            FrameKind::Keyframe | FrameKind::RectDelta => {
                latencies.push(frame_started.elapsed());
                payload_bytes += payload.len() as u64;
                raw_bytes += header.raw_bytes as u64;
            }
            FrameKind::Heartbeat => {}
            FrameKind::End => {
                return Err(AgentError::Command(
                    "framebuffer stream ended by producer".to_string(),
                ));
            }
            FrameKind::Error => {
                return Err(AgentError::Command(
                    String::from_utf8_lossy(&payload).into_owned(),
                ));
            }
            FrameKind::Hello => {
                return Err(AgentError::Protocol(
                    "unexpected framebuffer stream hello frame".to_string(),
                ));
            }
        }
    }
    Ok(FramebufferStreamDrainStats {
        latencies,
        payload_bytes,
        raw_bytes,
    })
}

fn request_framebuffer_stream_with_retry(
    client: &AgentClient,
) -> Result<(Value, BufReader<TcpStream>), AgentError> {
    retry_framebuffer_stream(
        || client.request_stream("framebuffer_stream_v1", json!({})),
        thread::sleep,
    )
}

fn retry_framebuffer_stream<T>(
    mut request: impl FnMut() -> Result<T, AgentError>,
    mut delay: impl FnMut(Duration),
) -> Result<T, AgentError> {
    let mut last_error = None;
    for attempt in 0..5 {
        match request() {
            Ok(stream) => return Ok(stream),
            Err(AgentError::Command(err))
                if err.contains("framebuffer stream already has a desktop consumer") =>
            {
                last_error = Some(AgentError::Command(err));
                delay(Duration::from_millis(50 * (attempt + 1)));
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AgentError::Command("framebuffer stream already has a desktop consumer".to_string())
    }))
}

struct AgentClient {
    host: String,
    token: String,
}

trait AgentConnector {
    type Transport: Read + Write;

    fn connect(&self, host: &str, read_timeout: Duration) -> Result<Self::Transport, AgentError>;
}

struct TcpAgentConnector;

impl AgentConnector for TcpAgentConnector {
    type Transport = TcpStream;

    fn connect(&self, host: &str, read_timeout: Duration) -> Result<Self::Transport, AgentError> {
        let addr = format!("{host}:{AGENT_PORT}")
            .to_socket_addrs()
            .map_err(|err| AgentError::Unreachable(err.to_string()))?
            .next()
            .ok_or_else(|| {
                AgentError::Unreachable("could not resolve MiSTer agent host".to_string())
            })?;
        let stream = TcpStream::connect_timeout(&addr, REQUEST_TIMEOUT)
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;
        stream
            .set_read_timeout(Some(read_timeout))
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;
        stream
            .set_write_timeout(Some(REQUEST_TIMEOUT))
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;
        Ok(stream)
    }
}

impl AgentClient {
    fn new(host: String, token: String) -> Self {
        Self { host, token }
    }

    fn request(&self, cmd: &str, args: Value) -> Result<Value, AgentError> {
        self.request_with_read_timeout(cmd, args, REQUEST_TIMEOUT)
    }

    fn request_with_read_timeout(
        &self,
        cmd: &str,
        args: Value,
        read_timeout: Duration,
    ) -> Result<Value, AgentError> {
        self.request_with_connector(cmd, args, read_timeout, &TcpAgentConnector)
    }

    fn request_binary(&self, cmd: &str, args: Value) -> Result<(Value, Vec<u8>), AgentError> {
        self.request_binary_with_connector(cmd, args, &TcpAgentConnector)
    }

    fn request_with_connector<C: AgentConnector>(
        &self,
        cmd: &str,
        args: Value,
        read_timeout: Duration,
        connector: &C,
    ) -> Result<Value, AgentError> {
        let mut transport = connector.connect(&self.host, read_timeout)?;
        let started = Instant::now();
        write_request(&mut transport, &self.token, cmd, args)?;
        let mut reader = BufReader::new(transport);
        read_response(&mut reader, started.elapsed())
    }

    fn request_binary_with_connector<C: AgentConnector>(
        &self,
        cmd: &str,
        args: Value,
        connector: &C,
    ) -> Result<(Value, Vec<u8>), AgentError> {
        let mut transport = connector.connect(&self.host, BINARY_REQUEST_TIMEOUT)?;
        write_request(&mut transport, &self.token, cmd, args)?;
        let mut reader = BufReader::new(transport);
        read_binary_response(&mut reader)
    }

    fn request_stream(
        &self,
        cmd: &str,
        args: Value,
    ) -> Result<(Value, BufReader<TcpStream>), AgentError> {
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
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;
        stream
            .set_write_timeout(Some(REQUEST_TIMEOUT))
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;

        write_request(&mut stream, &self.token, cmd, args)?;

        let mut reader = BufReader::new(stream);
        let value = read_response(&mut reader, Duration::ZERO)?;
        Ok((value, reader))
    }
}

fn write_request(
    writer: &mut impl Write,
    token: &str,
    cmd: &str,
    args: Value,
) -> Result<(), AgentError> {
    let request = agent_protocol::request(token, 1, cmd, args);
    writeln!(writer, "{request}").map_err(|err| AgentError::Unreachable(err.to_string()))?;
    writer
        .flush()
        .map_err(|err| AgentError::Unreachable(err.to_string()))
}

fn read_response(reader: &mut impl BufRead, elapsed: Duration) -> Result<Value, AgentError> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|err| AgentError::Unreachable(err.to_string()))?;
    parse_response(&line, elapsed)
}

fn read_binary_response(reader: &mut impl BufRead) -> Result<(Value, Vec<u8>), AgentError> {
    let value = read_response(reader, Duration::ZERO)?;
    let payload_bytes = binary_payload_len(&value)?;
    let mut payload = zeroed_buffer(payload_bytes, "binary response payload")?;
    reader
        .read_exact(&mut payload)
        .map_err(|err| AgentError::Unreachable(err.to_string()))?;
    Ok((value, payload))
}

fn binary_payload_len(value: &Value) -> Result<usize, AgentError> {
    agent_protocol::binary_payload_len(value).map_err(AgentError::Protocol)
}

fn reserved_buffer(len: usize, context: &str) -> Result<Vec<u8>, AgentError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|err| AgentError::Protocol(format!("allocate {context} ({len} bytes): {err}")))?;
    Ok(bytes)
}

fn zeroed_buffer(len: usize, context: &str) -> Result<Vec<u8>, AgentError> {
    let mut bytes = reserved_buffer(len, context)?;
    bytes.resize(len, 0);
    Ok(bytes)
}

fn decompress_size_prepended_exact(
    payload: &[u8],
    expected_raw: usize,
    max_raw: usize,
    context: &str,
) -> Result<Vec<u8>, AgentError> {
    if expected_raw > max_raw {
        return Err(AgentError::Protocol(format!(
            "{context} raw payload too large: {expected_raw} bytes"
        )));
    }
    let (prefixed_raw_len, compressed) = lz4_flex::block::uncompressed_size(payload)
        .map_err(|err| AgentError::Protocol(format!("decompress {context}: {err}")))?;
    if prefixed_raw_len != expected_raw {
        return Err(AgentError::Protocol(format!(
            "{context} LZ4 size prefix mismatch expected={expected_raw} actual={prefixed_raw_len}"
        )));
    }
    decompress_block_exact(compressed, expected_raw, max_raw, context)
}

fn decompress_block_exact(
    payload: &[u8],
    expected_raw: usize,
    max_raw: usize,
    context: &str,
) -> Result<Vec<u8>, AgentError> {
    if expected_raw > max_raw {
        return Err(AgentError::Protocol(format!(
            "{context} raw payload too large: {expected_raw} bytes"
        )));
    }
    let mut raw = zeroed_buffer(expected_raw, context)?;
    let decoded_len = lz4_flex::block::decompress_into(payload, &mut raw)
        .map_err(|err| AgentError::Protocol(format!("decompress {context}: {err}")))?;
    if decoded_len != expected_raw {
        return Err(AgentError::Protocol(format!(
            "{context} raw size mismatch expected={expected_raw} actual={decoded_len}"
        )));
    }
    Ok(raw)
}

impl FramebufferStream {
    pub fn control(&self) -> Result<FramebufferStreamControl, AgentError> {
        Ok(FramebufferStreamControl {
            stream: self
                .control
                .stream
                .try_clone()
                .map_err(|err| AgentError::Unreachable(err.to_string()))?,
        })
    }

    pub fn next_capture(&mut self) -> Result<FramebufferCapture, AgentError> {
        self.next_frame().map(|frame| frame.capture)
    }

    pub fn next_frame(&mut self) -> Result<FramebufferStreamFrame, AgentError> {
        loop {
            let read_started = Instant::now();
            let (header, payload) = read_frame(&mut self.reader).map_err(|err| {
                AgentError::Unreachable(format!("read framebuffer stream: {err}"))
            })?;
            let read_complete = Instant::now();
            match header.kind {
                FrameKind::Keyframe | FrameKind::RectDelta => {
                    if let Some(frame) = self.state.apply_frame_timed(
                        header,
                        &payload,
                        read_started,
                        read_complete,
                    )? {
                        return Ok(frame);
                    }
                }
                FrameKind::Heartbeat => continue,
                FrameKind::End => {
                    return Err(AgentError::Command(
                        "framebuffer stream ended by producer".to_string(),
                    ));
                }
                FrameKind::Error => {
                    return Err(AgentError::Command(
                        String::from_utf8_lossy(&payload).into_owned(),
                    ));
                }
                FrameKind::Hello => {
                    return Err(AgentError::Protocol(
                        "unexpected framebuffer stream hello frame".to_string(),
                    ));
                }
            }
        }
    }
}

impl DeviceTelemetryStream {
    pub fn control(&self) -> Result<DeviceTelemetryStreamControl, AgentError> {
        Ok(DeviceTelemetryStreamControl {
            stream: self
                .control
                .stream
                .try_clone()
                .map_err(|err| AgentError::Unreachable(err.to_string()))?,
        })
    }

    pub fn next_sample(&mut self) -> Result<DeviceTelemetrySample, AgentError> {
        let mut line = String::new();
        let bytes = self
            .reader
            .read_line(&mut line)
            .map_err(|err| AgentError::Unreachable(err.to_string()))?;
        if bytes == 0 {
            return Err(AgentError::Command("telemetry stream ended".to_string()));
        }
        parse_device_telemetry_sample(&line)
    }
}

impl DeviceTelemetryStreamControl {
    pub fn shutdown(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

impl FramebufferStreamControl {
    pub fn shutdown(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

impl FramebufferStreamState {
    #[cfg(test)]
    fn apply_frame(
        &mut self,
        header: FrameHeader,
        payload: &[u8],
    ) -> Result<Option<FramebufferStreamFrame>, AgentError> {
        let now = Instant::now();
        self.apply_frame_timed(header, payload, now, now)
    }

    fn apply_frame_timed(
        &mut self,
        header: FrameHeader,
        payload: &[u8],
        read_started: Instant,
        read_complete: Instant,
    ) -> Result<Option<FramebufferStreamFrame>, AgentError> {
        if header.flags != FLAG_LZ4_SIZE_PREPENDED {
            return Err(AgentError::Protocol(format!(
                "framebuffer stream frame has unsupported flags 0x{:04x}",
                header.flags
            )));
        }
        if !matches!(header.kind, FrameKind::Keyframe | FrameKind::RectDelta) {
            return Err(AgentError::Protocol(format!(
                "unexpected framebuffer stream frame kind: {:?}",
                header.kind
            )));
        }
        header
            .validate_shape()
            .map_err(|err| AgentError::Protocol(err.to_string()))?;
        if self.awaiting_keyframe && header.kind != FrameKind::Keyframe {
            return Ok(None);
        }
        if header.kind == FrameKind::RectDelta {
            if self.geometry != Some(header.geometry) {
                self.awaiting_keyframe = true;
                self.expected_sequence = None;
                return Ok(None);
            }
            if let Some(expected) = self.expected_sequence {
                if expected != header.sequence {
                    self.awaiting_keyframe = true;
                    self.expected_sequence = None;
                    return Ok(None);
                }
            }
        }
        let raw = decompress_size_prepended_exact(
            payload,
            header.raw_bytes as usize,
            MAX_FRAME_SURFACE_BYTES,
            "framebuffer stream",
        )?;
        let decompress_complete = Instant::now();
        if self.geometry != Some(header.geometry) || header.kind == FrameKind::Keyframe {
            self.reset_buffer(header.geometry)?;
        }
        apply_rgb565_rect(&mut self.rgb565, header.geometry, header.rect, &raw)?;
        self.geometry = Some(header.geometry);
        self.expected_sequence = Some(header.sequence.saturating_add(1));
        self.awaiting_keyframe = false;
        let stride_bytes = header
            .geometry
            .stride_pixels
            .checked_mul(2)
            .ok_or_else(|| AgentError::Protocol("framebuffer stream stride overflow".to_string()))?
            as u64;
        let rgba_pixels = framebuffer_raw_to_rgba(
            &self.rgb565,
            header.geometry.width as u64,
            header.geometry.height as u64,
            stride_bytes,
            16,
        )?;
        let rgba_complete = Instant::now();
        let capture = FramebufferCapture {
            png_path: PathBuf::new(),
            rgba_pixels,
            raw_pixels: Vec::new(),
            raw_stride_bytes: 0,
            width: header.geometry.width as u64,
            height: header.geometry.height as u64,
            bpp: 16,
            raw_bytes: header.raw_bytes as u64,
            payload_bytes: header.payload_bytes as u64,
            encoding: "framebuffer-stream-v1/lz4-block-size-prepended".to_string(),
            png_bytes: 0,
            png_hex_bytes: 0,
            timing: FramebufferCaptureTiming::default(),
        };
        Ok(Some(FramebufferStreamFrame {
            capture,
            kind: header.kind,
            sequence: header.sequence,
            timestamp_us: header.timestamp_us,
            geometry: header.geometry,
            rect: header.rect,
            raw_bytes: header.raw_bytes as u64,
            payload_bytes: header.payload_bytes as u64,
            timing: FramebufferStreamTiming {
                read_started,
                read_complete,
                decompress_complete,
                rgba_complete,
            },
        }))
    }

    fn reset_buffer(&mut self, geometry: FrameGeometry) -> Result<(), AgentError> {
        let bytes = geometry
            .stride_pixels
            .checked_mul(geometry.height)
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| {
                AgentError::Protocol("framebuffer stream geometry overflow".to_string())
            })? as usize;
        if bytes > MAX_FRAME_SURFACE_BYTES {
            return Err(AgentError::Protocol(format!(
                "framebuffer stream surface too large: {bytes} bytes"
            )));
        }
        self.rgb565 = zeroed_buffer(bytes, "framebuffer stream surface")?;
        self.expected_sequence = None;
        self.awaiting_keyframe = false;
        Ok(())
    }

    fn seed_from_capture(&mut self, capture: &FramebufferCapture) -> Result<(), AgentError> {
        if capture.bpp != 16 || capture.raw_pixels.is_empty() {
            return Ok(());
        }
        if capture.width == 0 || capture.height == 0 || capture.raw_stride_bytes == 0 {
            return Ok(());
        }
        if !capture.raw_stride_bytes.is_multiple_of(2) {
            return Err(AgentError::Protocol(
                "framebuffer capture stride is not 16bpp aligned".to_string(),
            ));
        }
        let stride_pixels = capture.raw_stride_bytes / 2;
        if stride_pixels < capture.width {
            return Err(AgentError::Protocol(
                "framebuffer capture stride is smaller than width".to_string(),
            ));
        }
        let expected = capture
            .raw_stride_bytes
            .checked_mul(capture.height)
            .ok_or_else(|| AgentError::Protocol("framebuffer seed size overflow".to_string()))?
            as usize;
        if capture.raw_pixels.len() != expected {
            return Err(AgentError::Protocol(format!(
                "framebuffer seed size mismatch expected={expected} actual={}",
                capture.raw_pixels.len()
            )));
        }
        let geometry = FrameGeometry {
            width: u32::try_from(capture.width).map_err(|_| {
                AgentError::Protocol("framebuffer seed width too large".to_string())
            })?,
            height: u32::try_from(capture.height).map_err(|_| {
                AgentError::Protocol("framebuffer seed height too large".to_string())
            })?,
            stride_pixels: u32::try_from(stride_pixels).map_err(|_| {
                AgentError::Protocol("framebuffer seed stride too large".to_string())
            })?,
        };
        geometry.validate_seed_shape()?;
        self.rgb565 = capture.raw_pixels.clone();
        self.geometry = Some(geometry);
        self.expected_sequence = None;
        self.awaiting_keyframe = false;
        Ok(())
    }
}

trait FrameGeometrySeedExt {
    fn validate_seed_shape(self) -> Result<(), AgentError>;
}

impl FrameGeometrySeedExt for FrameGeometry {
    fn validate_seed_shape(self) -> Result<(), AgentError> {
        if self.width == 0 || self.height == 0 || self.stride_pixels < self.width {
            return Err(AgentError::Protocol(
                "invalid framebuffer seed geometry".to_string(),
            ));
        }
        Ok(())
    }
}

fn apply_rgb565_rect(
    framebuffer: &mut [u8],
    geometry: FrameGeometry,
    rect: FrameRect,
    raw: &[u8],
) -> Result<(), AgentError> {
    let row_bytes = rect
        .width
        .checked_mul(2)
        .ok_or_else(|| AgentError::Protocol("framebuffer stream rect overflow".to_string()))?
        as usize;
    if raw.len()
        != row_bytes
            .checked_mul(rect.height as usize)
            .ok_or_else(|| AgentError::Protocol("framebuffer stream raw overflow".to_string()))?
    {
        return Err(AgentError::Protocol(
            "framebuffer stream rect payload length mismatch".to_string(),
        ));
    }
    let stride_bytes = geometry
        .stride_pixels
        .checked_mul(2)
        .ok_or_else(|| AgentError::Protocol("framebuffer stream stride overflow".to_string()))?
        as usize;
    let x_bytes = rect
        .x
        .checked_mul(2)
        .ok_or_else(|| AgentError::Protocol("framebuffer stream rect overflow".to_string()))?
        as usize;
    for row in 0..rect.height as usize {
        let src = row * row_bytes;
        let dst = (rect.y as usize + row)
            .checked_mul(stride_bytes)
            .and_then(|offset| offset.checked_add(x_bytes))
            .ok_or_else(|| {
                AgentError::Protocol("framebuffer stream offset overflow".to_string())
            })?;
        let dst_end = dst.checked_add(row_bytes).ok_or_else(|| {
            AgentError::Protocol("framebuffer stream offset overflow".to_string())
        })?;
        let Some(dst_row) = framebuffer.get_mut(dst..dst_end) else {
            return Err(AgentError::Protocol(
                "framebuffer stream rect outside framebuffer".to_string(),
            ));
        };
        dst_row.copy_from_slice(&raw[src..src + row_bytes]);
    }
    Ok(())
}

fn parse_response(line: &str, _elapsed: Duration) -> Result<Value, AgentError> {
    match agent_protocol::parse_response_line(line).map_err(AgentError::Protocol)? {
        ResponseEnvelope::Ok { result, .. } => Ok(result),
        ResponseEnvelope::Error(error) if error == "unauthorized" => Err(AgentError::Unauthorized),
        ResponseEnvelope::Error(error) => Err(AgentError::Command(error)),
    }
}

fn parse_device_telemetry_sample(line: &str) -> Result<DeviceTelemetrySample, AgentError> {
    let value: Value = serde_json::from_str(line.trim())
        .map_err(|err| AgentError::Protocol(format!("invalid telemetry JSON: {err}")))?;
    if value.get("schema").and_then(Value::as_str) != Some("mister-magik-device-telemetry-v2") {
        return Err(AgentError::Protocol(
            "unexpected telemetry schema".to_string(),
        ));
    }
    let presentation = value.pointer("/presentation").unwrap_or(&Value::Null);
    if presentation.get("schema").and_then(Value::as_str)
        != Some("mister-magik-presentation-telemetry-snapshot-v1")
        || presentation.get("source").and_then(Value::as_str)
            != Some("fpga-owned-vblank-telemetry")
    {
        return Err(AgentError::Protocol(
            "missing authoritative FPGA presentation telemetry".to_string(),
        ));
    }
    let frame = value
        .pointer("/launcher/frame_budget")
        .unwrap_or(&Value::Null);
    Ok(DeviceTelemetrySample {
        seq: u64_at(&value, "/seq"),
        combined_cpu_pct: f64_at(&value, "/cpu/combined_busy_pct"),
        presentation: PresentationTelemetrySample {
            available: presentation
                .get("available")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            captured_monotonic_us: u64_at(presentation, "/captured_monotonic_us"),
            owned_vblank_count: optional_u32_at(presentation, "/owned_vblank_count"),
            presented_vblank_count: optional_u32_at(presentation, "/presented_vblank_count"),
            repeated_vblank_count: optional_u32_at(presentation, "/repeated_vblank_count"),
            ownership_loss_count: optional_u32_at(presentation, "/ownership_loss_count"),
            active_sequence: optional_u16_at(presentation, "/active_sequence"),
            magik_ownership: presentation
                .get("magik_ownership")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            pending: presentation
                .get("pending")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            lifetime_invariant_valid: presentation
                .get("lifetime_invariant_valid")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            error: str_at(presentation, "/error", "").to_string(),
        },
        cores: value
            .pointer("/cpu/cores")
            .and_then(Value::as_array)
            .map(|cores| {
                cores
                    .iter()
                    .map(|core| CpuCoreTelemetry {
                        label: format!("CPU{}", u64_at(core, "/id")),
                        busy_pct: f64_at(core, "/busy_pct"),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        memory: MemoryTelemetry {
            total_kb: u64_at(&value, "/memory/total_kb"),
            magik_kb: u64_at(&value, "/memory/magik_kb"),
            main_kb: u64_at(&value, "/memory/main_kb"),
            other_used_kb: u64_at(&value, "/memory/other_used_kb"),
            available_kb: u64_at(&value, "/memory/available_kb"),
            magik_pct: f64_at(&value, "/memory/magik_pct"),
            other_used_pct: f64_at(&value, "/memory/other_used_pct"),
            available_pct: f64_at(&value, "/memory/available_pct"),
        },
        frame_budget: FrameBudgetTelemetry {
            budget_us: u64_at(frame, "/budget_us").max(16_667),
            frames_total: u64_at(frame, "/frames_total"),
            window_frames: u64_at(frame, "/window_frames"),
            window_over_budget: u64_at(frame, "/window_over_budget"),
            window_over_20ms: u64_at(frame, "/window_over_20ms"),
            window_over_33ms: u64_at(frame, "/window_over_33ms"),
            window_max_wall_us: u64_at(frame, "/window_max_wall_us"),
            max_wall_us: u64_at(frame, "/max_wall_us"),
            max_vsync_miss_streak: u64_at(frame, "/max_vsync_miss_streak"),
            window_prepare_us: u64_at(frame, "/window_prepare_us"),
            window_render_us: u64_at(frame, "/window_render_us"),
            window_custom_draw_us: u64_at(frame, "/window_custom_draw_us"),
            window_vsync_us: u64_at(frame, "/window_vsync_us"),
            window_present_us: u64_at(frame, "/window_present_us"),
            recent_frames: frame
                .pointer("/recent_frames")
                .and_then(Value::as_array)
                .map(|frames| frames.iter().map(parse_frame_budget_recent_frame).collect())
                .unwrap_or_default(),
        },
        launcher: LauncherTelemetry {
            status_current: value
                .pointer("/launcher/status_current")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            idle: value
                .pointer("/launcher/idle")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            fps: value
                .pointer("/launcher/rolling_fps")
                .or_else(|| value.pointer("/launcher/fps_estimate"))
                .and_then(Value::as_f64)
                .map(|fps| format!("{fps:.1} fps"))
                .unwrap_or_else(|| "- fps".to_string()),
            preview_cache_state: str_at(&value, "/launcher/preview_cache_state", "unknown"),
            ui_thread_cpu: value
                .pointer("/launcher/ui_thread_cpu")
                .and_then(Value::as_u64),
        },
        magik: process_telemetry_at(&value, "/processes/mister-magik-fb"),
        main: main_process_telemetry(&value),
        network: NetworkTelemetry {
            rx_bytes_per_sec: u64_at(&value, "/network/rx_bytes_per_sec"),
            tx_bytes_per_sec: u64_at(&value, "/network/tx_bytes_per_sec"),
        },
        storage: StorageTelemetry {
            available_bytes: u64_at(&value, "/storage/available_bytes"),
            total_bytes: u64_at(&value, "/storage/total_bytes"),
            available_pct: f64_at(&value, "/storage/available_pct"),
            device: string_at(&value, "/storage/device")
                .unwrap_or_default()
                .to_string(),
            activity_valid: value
                .pointer("/storage/activity_valid")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            read_bytes_per_sec: u64_at(&value, "/storage/read_bytes_per_sec"),
            write_bytes_per_sec: u64_at(&value, "/storage/write_bytes_per_sec"),
            read_pct: f64_at(&value, "/storage/read_pct"),
            write_pct: f64_at(&value, "/storage/write_pct"),
        },
    })
}

fn optional_u32_at(value: &Value, pointer: &str) -> Option<u32> {
    value.pointer(pointer)?.as_u64()?.try_into().ok()
}

fn optional_u16_at(value: &Value, pointer: &str) -> Option<u16> {
    value.pointer(pointer)?.as_u64()?.try_into().ok()
}

fn parse_frame_budget_recent_frame(value: &Value) -> FrameBudgetFrameTelemetry {
    FrameBudgetFrameTelemetry {
        frame: u64_at(value, "/frame"),
        wall_us: u64_at(value, "/wall_us"),
        prepare_us: u64_at(value, "/prepare_us"),
        render_us: u64_at(value, "/render_us"),
        custom_draw_us: u64_at(value, "/custom_draw_us"),
        vsync_us: u64_at(value, "/vsync_us"),
        present_us: u64_at(value, "/present_us"),
        cpu_prepare_us: u64_at(value, "/cpu_prepare_us"),
        cpu_render_us: u64_at(value, "/cpu_render_us"),
        cpu_custom_draw_us: u64_at(value, "/cpu_custom_draw_us"),
        cpu_vsync_us: u64_at(value, "/cpu_vsync_us"),
        cpu_present_us: u64_at(value, "/cpu_present_us"),
        process_cpu_us: u64_at(value, "/process_cpu_us"),
        vsync_source: value
            .pointer("/vsync_source")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        vsync_miss_streak: u64_at(value, "/vsync_miss_streak"),
    }
}

fn process_telemetry_at(value: &Value, pointer: &str) -> ProcessTelemetry {
    let item = value.pointer(pointer).unwrap_or(&Value::Null);
    ProcessTelemetry {
        pids: item
            .get("pids")
            .and_then(Value::as_array)
            .map(|pids| pids.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default(),
        rss_kb: item.get("rss_kb").and_then(Value::as_u64).unwrap_or(0),
        threads: item.get("threads").and_then(Value::as_u64).unwrap_or(0),
    }
}

fn main_process_telemetry(value: &Value) -> ProcessTelemetry {
    let dev = process_telemetry_at(value, "/processes/MiSTer_MagiKDev");
    if dev.pids.is_empty() {
        process_telemetry_at(value, "/processes/MiSTer_MagiK")
    } else {
        dev
    }
}

fn main_process_summary(value: &Value) -> String {
    let dev_running = value
        .pointer("/processes/MiSTer_MagiKDev")
        .and_then(Value::as_array)
        .is_some_and(|pids| !pids.is_empty());
    process_summary(
        value,
        if dev_running {
            "MiSTer_MagiKDev"
        } else {
            "MiSTer_MagiK"
        },
    )
}

fn u64_at(value: &Value, pointer: &str) -> u64 {
    value.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

fn f64_at(value: &Value, pointer: &str) -> f64 {
    value
        .pointer(pointer)
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

fn str_at(value: &Value, pointer: &str, fallback: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_string()
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
    snapshot.main_process = main_process_summary(status);
    snapshot.launcher_process = process_summary(status, "mister-magik-fb");
    let scanout_slots = status.pointer("/scanout_slots").unwrap_or(&Value::Null);
    (
        snapshot.scanout_slots_summary,
        snapshot.scanout_slots_detail,
    ) = crate::app_state::scanout_slots_labels(
        scanout_slots
            .pointer("/module_loaded")
            .and_then(Value::as_bool),
        scanout_slots
            .pointer("/device_ready")
            .and_then(Value::as_bool),
    );
}

fn apply_magik_status(snapshot: &mut DashboardSnapshot, status: &Value) {
    snapshot.main_process = main_process_summary(status);
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
    (
        snapshot.scanout_slots_summary,
        snapshot.scanout_slots_detail,
    ) = crate::app_state::scanout_slots_labels(
        main_status
            .pointer("/scanout_slots_module_loaded")
            .and_then(Value::as_bool),
        main_status
            .pointer("/scanout_slots_device_ready")
            .and_then(Value::as_bool),
    );
}

fn parse_sd_directory(value: &Value) -> Result<SdDirectoryListing, AgentError> {
    if !matches!(
        string_at(value, "/schema"),
        Some("mister-magik-sd-list-dir-v1" | "mister-magik-sd-list-dir-v2")
    ) {
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
        round_trip_ms: 0,
    })
}

fn parse_sd_item_detail(value: &Value) -> Result<SdItemDetail, AgentError> {
    if string_at(value, "/schema") != Some("mister-magik-sd-stat-item-v1") {
        return Err(AgentError::Protocol(
            "unexpected sd_stat_item response schema".to_string(),
        ));
    }
    let path = string_at(value, "/path")
        .ok_or_else(|| AgentError::Protocol("missing sd item path".to_string()))?
        .to_string();
    let fallback_name = item_name(&path);
    let name = string_at(value, "/name").unwrap_or(fallback_name.as_str());
    let kind = string_at(value, "/kind").unwrap_or("file");
    let extension = string_at(value, "/extension").unwrap_or("");
    let size = value.pointer("/size").and_then(Value::as_u64).unwrap_or(0);
    let modified_unix_ms = value
        .pointer("/modified_unix_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let readonly = value
        .pointer("/readonly")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let hidden = value
        .pointer("/hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let has_image = value
        .pointer("/capabilities/image_preview")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let is_mra = value
        .pointer("/capabilities/mra_parse")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut overview_rows = vec![
        metadata_row("Path", &path, "path"),
        metadata_row("Type", kind, "text"),
        metadata_row(
            "Extension",
            if extension.is_empty() { "-" } else { extension },
            "text",
        ),
        metadata_row("Size", &format_file_size(size), "text"),
        metadata_row("Modified", &format_unix_ms(modified_unix_ms), "text"),
        metadata_row(
            "Readonly",
            yes_no(readonly),
            if readonly { "warning" } else { "text" },
        ),
        metadata_row(
            "Hidden",
            yes_no(hidden),
            if hidden { "warning" } else { "text" },
        ),
    ];
    add_planned_capability_rows(value, &mut overview_rows);
    Ok(SdItemDetail {
        path,
        title: name.to_string(),
        subtitle: if kind == "directory" {
            "Folder on /media/fat".to_string()
        } else {
            format!("{} file on /media/fat", extension.to_uppercase())
        },
        kind: kind.to_string(),
        icon_key: if kind == "directory" {
            "folder-base".to_string()
        } else {
            crate::sd_card::material_icon_key_for_file_name(name).to_string()
        },
        size_label: format_file_size(size),
        modified_label: format_unix_ms(modified_unix_ms),
        flags_label: flags_label(readonly, hidden),
        loading: false,
        error: String::new(),
        has_image,
        image_path: String::new(),
        image_summary: String::new(),
        is_mra,
        overview_rows,
        mra_summary_rows: Vec::new(),
        mra_xml_rows: Vec::new(),
        mra_path_rows: Vec::new(),
        mra_warnings: Vec::new(),
        raw_xml: String::new(),
        raw_xml_truncated: false,
    })
}

fn apply_sd_preview_image(
    detail: &mut SdItemDetail,
    value: &Value,
    payload: &[u8],
) -> Result<(), AgentError> {
    if string_at(value, "/schema") != Some("mister-magik-sd-preview-image-v1") {
        return Err(AgentError::Protocol(
            "unexpected sd preview image schema".to_string(),
        ));
    }
    let expected = value
        .pointer("/payload_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(payload.len() as u64);
    if payload.len() as u64 != expected {
        return Err(AgentError::Protocol(format!(
            "preview payload size mismatch expected={expected} actual={}",
            payload.len()
        )));
    }
    let format = string_at(value, "/format").unwrap_or("image");
    let width = value.pointer("/width").and_then(Value::as_u64).unwrap_or(0);
    let height = value
        .pointer("/height")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let image_path = local_sd_preview_path(&detail.path, format);
    fs::write(&image_path, payload)
        .map_err(|err| AgentError::Unreachable(format!("write preview image: {err}")))?;
    detail.image_path = image_path.to_string_lossy().to_string();
    detail.image_summary = format!("{format} {width}x{height}, {}", format_file_size(expected));
    detail
        .overview_rows
        .push(metadata_row("Image", &detail.image_summary, "success"));
    Ok(())
}

fn apply_sd_mra_detail(detail: &mut SdItemDetail, value: &Value) -> Result<(), AgentError> {
    if string_at(value, "/schema") != Some("mister-magik-sd-parse-mra-v1") {
        return Err(AgentError::Protocol(
            "unexpected sd_parse_mra response schema".to_string(),
        ));
    }
    detail.mra_summary_rows = parse_metadata_array(value.pointer("/summary"), "text");
    detail.mra_xml_rows = parse_xml_row_array(value.pointer("/xml_rows"));
    detail.mra_path_rows = parse_xml_row_array(value.pointer("/path_rows"));
    detail.mra_warnings = value
        .pointer("/warnings")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(|warning| metadata_row("Warning", warning, "warning"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    detail.raw_xml = string_at(value, "/raw_xml").unwrap_or("").to_string();
    detail.raw_xml_truncated = value
        .pointer("/raw_xml_truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    detail.overview_rows.push(metadata_row(
        "MRA XML rows",
        &detail.mra_xml_rows.len().to_string(),
        "success",
    ));
    Ok(())
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
        raw_pixels: Vec::new(),
        raw_stride_bytes: 0,
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
    if string_at(value, "/schema") != Some("mister-magik-framebuffer-raw-stream-v2") {
        return Err(AgentError::Protocol(
            "unexpected framebuffer raw stream response schema".to_string(),
        ));
    }
    let source = string_at(value, "/source")
        .ok_or_else(|| AgentError::Protocol("missing framebuffer capture source".to_string()))?;
    let source_kind = string_at(value, "/capture_source/kind").ok_or_else(|| {
        AgentError::Protocol("missing framebuffer capture_source.kind".to_string())
    })?;
    if source != source_kind
        || !matches!(
            source,
            "fb0" | "producer-composition" | "fpga-latched-scanout-slots"
        )
    {
        return Err(AgentError::Protocol(format!(
            "invalid framebuffer capture source: {source}"
        )));
    }
    let authoritative = value
        .pointer("/authoritative_scanout")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            AgentError::Protocol("missing framebuffer authoritative_scanout".to_string())
        })?;
    if authoritative != (source == "fpga-latched-scanout-slots") {
        return Err(AgentError::Protocol(
            "inconsistent framebuffer scanout authority".to_string(),
        ));
    }
    let encoding = string_at(value, "/encoding").unwrap_or("raw");
    let expected_raw = value
        .pointer("/raw_bytes")
        .and_then(Value::as_u64)
        .map(usize::try_from)
        .transpose()
        .map_err(|_| AgentError::Protocol("framebuffer raw size overflows usize".to_string()))?;
    let raw = match encoding {
        "lz4-block-size-prepended" => decompress_size_prepended_exact(
            payload,
            expected_raw
                .ok_or_else(|| AgentError::Protocol("missing framebuffer raw_bytes".to_string()))?,
            MAX_FRAME_SURFACE_BYTES,
            "framebuffer capture",
        )?,
        "raw" => {
            if payload.len() > MAX_FRAME_SURFACE_BYTES {
                return Err(AgentError::Protocol(format!(
                    "framebuffer capture raw payload too large: {} bytes",
                    payload.len()
                )));
            }
            payload.to_vec()
        }
        other => {
            return Err(AgentError::Protocol(format!(
                "unsupported framebuffer raw stream encoding: {other}"
            )));
        }
    };
    let width = value.pointer("/width").and_then(Value::as_u64).unwrap_or(0);
    let height = value
        .pointer("/height")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let stride = value
        .pointer("/stride")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let bpp = value.pointer("/bpp").and_then(Value::as_u64).unwrap_or(0);
    let expected_raw = expected_raw.unwrap_or(raw.len());
    if raw.len() != expected_raw {
        return Err(AgentError::Protocol(format!(
            "decoded framebuffer size mismatch expected={expected_raw} actual={}",
            raw.len()
        )));
    }
    let raw_len = raw.len() as u64;
    let rgba_pixels = framebuffer_raw_to_rgba(&raw, width, height, stride, bpp)?;
    Ok(FramebufferCapture {
        png_path: PathBuf::new(),
        rgba_pixels,
        raw_pixels: raw,
        raw_stride_bytes: stride,
        width,
        height,
        bpp,
        raw_bytes: raw_len,
        payload_bytes: payload.len() as u64,
        encoding: encoding.to_string(),
        png_bytes: 0,
        png_hex_bytes: 0,
        timing: parse_framebuffer_capture_timing(value),
    })
}

pub(crate) fn parse_library_database_snapshot_lz4(
    value: &Value,
    payload: &[u8],
) -> Result<LibraryDatabaseSnapshot, AgentError> {
    if string_at(value, "/schema") != Some("mister-magik-library-db-snapshot-v1") {
        return Err(AgentError::Protocol(
            "unexpected library database snapshot schema".to_string(),
        ));
    }
    let remote_path = string_at(value, "/remote_path")
        .ok_or_else(|| AgentError::Protocol("missing library snapshot remote_path".to_string()))?
        .to_string();
    if remote_path != "/media/fat/mister-magik-dev/library.sqlite3" {
        return Err(AgentError::Protocol(
            "library snapshot remote_path is not allowlisted".to_string(),
        ));
    }
    let encoding = string_at(value, "/encoding").unwrap_or("");
    if encoding != "lz4-block" {
        return Err(AgentError::Protocol(format!(
            "unsupported library snapshot encoding: {encoding}"
        )));
    }
    let raw_bytes = value
        .pointer("/raw_bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| AgentError::Protocol("missing library snapshot raw_bytes".to_string()))?;
    let payload_bytes = value
        .pointer("/payload_bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            AgentError::Protocol("missing library snapshot payload_bytes".to_string())
        })?;
    if payload.len() as u64 != payload_bytes {
        return Err(AgentError::Protocol(format!(
            "library snapshot payload size mismatch expected={payload_bytes} actual={}",
            payload.len()
        )));
    }
    let checksum = string_at(value, "/checksum")
        .ok_or_else(|| AgentError::Protocol("missing library snapshot checksum".to_string()))?
        .to_string();
    let raw_len = usize::try_from(raw_bytes)
        .map_err(|_| AgentError::Protocol("library snapshot is too large".to_string()))?;
    let bytes = decompress_block_exact(
        payload,
        raw_len,
        MAX_AGENT_BINARY_PAYLOAD_BYTES as usize,
        "library snapshot LZ4",
    )?;
    let actual_checksum = fnv64_hex(&bytes);
    if actual_checksum != checksum {
        return Err(AgentError::Protocol(format!(
            "library snapshot checksum mismatch expected={checksum} actual={actual_checksum}"
        )));
    }
    Ok(LibraryDatabaseSnapshot {
        remote_path,
        bytes,
        raw_bytes,
        payload_bytes,
        checksum,
        mtime_unix_ms: value
            .pointer("/mtime_unix_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
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
            )));
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

    let rgba_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| AgentError::Protocol("RGBA image size overflow".to_string()))?;
    let mut rgba = reserved_buffer(rgba_len, "RGBA image")?;
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

pub(crate) fn fnv64_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
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
            )));
        }
        None => return Err(AgentError::Protocol("missing sd entry kind".to_string())),
    };
    Ok(SdEntry { name, path, kind })
}

fn metadata_row(label: &str, value: &str, kind: &str) -> SdMetadataRow {
    SdMetadataRow {
        label: label.to_string(),
        value: if value.is_empty() {
            "-".to_string()
        } else {
            value.to_string()
        },
        kind: kind.to_string(),
    }
}

fn parse_metadata_array(value: Option<&Value>, default_kind: &str) -> Vec<SdMetadataRow> {
    value
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    metadata_row(
                        string_at(row, "/label").unwrap_or("-"),
                        string_at(row, "/value").unwrap_or("-"),
                        string_at(row, "/kind").unwrap_or(default_kind),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_xml_row_array(value: Option<&Value>) -> Vec<SdMetadataRow> {
    value
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let order = row.pointer("/order").and_then(Value::as_u64).unwrap_or(0);
                    let depth = row.pointer("/depth").and_then(Value::as_u64).unwrap_or(0);
                    let kind = string_at(row, "/kind").unwrap_or("xml");
                    let path = string_at(row, "/path").unwrap_or("-");
                    let name = string_at(row, "/name").unwrap_or("");
                    let value = string_at(row, "/value").unwrap_or("");
                    let label = if name.is_empty() {
                        format!("{order:04} d{depth} {kind}")
                    } else {
                        format!("{order:04} d{depth} {kind} {name}")
                    };
                    let display = if value.is_empty() {
                        path.to_string()
                    } else {
                        format!("{path} = {value}")
                    };
                    metadata_row(&label, &display, kind)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn add_planned_capability_rows(value: &Value, rows: &mut Vec<SdMetadataRow>) {
    for (label, pointer) in [
        ("PNG/JPEG preview", "/capabilities/image_preview"),
        ("MRA full XML parse", "/capabilities/mra_parse"),
        ("INI summary", "/capabilities/ini_summary"),
        ("RBF summary", "/capabilities/rbf_summary"),
        ("Save-file hint", "/capabilities/save_hint"),
        ("Archive summary", "/capabilities/archive_summary"),
        ("SQLite summary", "/capabilities/sqlite_summary"),
        ("Folder analysis", "/capabilities/folder_analysis"),
    ] {
        if value
            .pointer(pointer)
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            rows.push(metadata_row(label, "Available", "success"));
        }
    }
}

fn format_file_size(bytes: u64) -> String {
    const UNITS: &[(&str, u64)] = &[
        ("GiB", 1024 * 1024 * 1024),
        ("MiB", 1024 * 1024),
        ("KiB", 1024),
    ];
    for (unit, factor) in UNITS {
        if bytes >= *factor {
            let value = bytes as f64 / *factor as f64;
            return format!("{value:.1} {unit} ({bytes} bytes)");
        }
    }
    format!("{bytes} bytes")
}

fn format_unix_ms(ms: u64) -> String {
    if ms == 0 {
        "-".to_string()
    } else {
        format!("{ms} ms since Unix epoch")
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn flags_label(readonly: bool, hidden: bool) -> String {
    let mut flags = Vec::new();
    if readonly {
        flags.push("readonly");
    }
    if hidden {
        flags.push("hidden");
    }
    if flags.is_empty() {
        "normal".to_string()
    } else {
        flags.join(", ")
    }
}

fn local_sd_preview_path(path: &str, format: &str) -> PathBuf {
    let safe = path
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    env::temp_dir().join(format!(
        "mister-magik-sd-preview-{}-{}.{}",
        std::process::id(),
        safe,
        if format == "jpeg" { "jpg" } else { format }
    ))
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
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::sync::Arc;
    use std::sync::Mutex;

    struct ScriptedTransport {
        response: Cursor<Vec<u8>>,
        writes: Arc<Mutex<Vec<u8>>>,
    }

    impl Read for ScriptedTransport {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.response.read(buffer)
        }
    }

    impl Write for ScriptedTransport {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.writes.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct ScriptedConnector {
        responses: Mutex<VecDeque<Vec<u8>>>,
        writes: Arc<Mutex<Vec<u8>>>,
    }

    impl ScriptedConnector {
        fn new(responses: impl IntoIterator<Item = Vec<u8>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                writes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn written_json(&self) -> Value {
            let bytes = self.writes.lock().unwrap();
            serde_json::from_slice(bytes.as_slice()).unwrap()
        }
    }

    impl AgentConnector for ScriptedConnector {
        type Transport = ScriptedTransport;

        fn connect(
            &self,
            _host: &str,
            _read_timeout: Duration,
        ) -> Result<Self::Transport, AgentError> {
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| AgentError::Unreachable("no scripted transport".into()))?;
            Ok(ScriptedTransport {
                response: Cursor::new(response),
                writes: Arc::clone(&self.writes),
            })
        }
    }

    #[test]
    fn in_memory_transport_round_trips_request_and_response() {
        let connector = ScriptedConnector::new([br#"{"id":1,"ok":true,"result":{"pong":true}}
"#
        .to_vec()]);
        let client = AgentClient::new("unused.test".into(), "secret".into());
        let result = client
            .request_with_connector(
                "ping",
                json!({"detail": true}),
                Duration::from_secs(9),
                &connector,
            )
            .unwrap();
        assert_eq!(result, json!({"pong": true}));
        assert_eq!(
            connector.written_json(),
            json!({
                "token": "secret",
                "id": 1,
                "cmd": "ping",
                "args": {"detail": true}
            })
        );
    }

    #[test]
    fn in_memory_transport_classifies_empty_malformed_auth_and_command_responses() {
        for (response, expected) in [
            (b"".as_slice(), "protocol"),
            (b"not-json\n".as_slice(), "protocol"),
            (
                br#"{"id":1,"ok":false,"error":"unauthorized"}
"#,
                "unauthorized",
            ),
            (
                br#"{"id":1,"ok":false,"error":"bad command"}
"#,
                "command",
            ),
        ] {
            let connector = ScriptedConnector::new([response.to_vec()]);
            let client = AgentClient::new("unused.test".into(), "secret".into());
            let error = client
                .request_with_connector("fixture", json!({}), Duration::ZERO, &connector)
                .unwrap_err();
            assert!(matches!(
                (expected, error),
                ("protocol", AgentError::Protocol(_))
                    | ("unauthorized", AgentError::Unauthorized)
                    | ("command", AgentError::Command(_))
            ));
        }
    }

    #[test]
    fn in_memory_binary_transport_reads_exact_payload_and_rejects_truncation() {
        let connector =
            ScriptedConnector::new([br#"{"id":1,"ok":true,"result":{"payload_bytes":4}}
data"#
                .to_vec()]);
        let client = AgentClient::new("unused.test".into(), "secret".into());
        let (metadata, payload) = client
            .request_binary_with_connector("binary", json!({}), &connector)
            .unwrap();
        assert_eq!(metadata["payload_bytes"], 4);
        assert_eq!(payload, b"data");

        let connector =
            ScriptedConnector::new([br#"{"id":1,"ok":true,"result":{"payload_bytes":5}}
tiny"#
                .to_vec()]);
        let error = client
            .request_binary_with_connector("binary", json!({}), &connector)
            .unwrap_err();
        assert!(matches!(error, AgentError::Unreachable(_)));
    }

    #[test]
    fn framebuffer_busy_retry_is_bounded_and_skips_delay_for_other_failures() {
        let busy = || {
            Err::<(), _>(AgentError::Command(
                "framebuffer stream already has a desktop consumer".into(),
            ))
        };
        let mut delays = Vec::new();
        let error = retry_framebuffer_stream(busy, |delay| delays.push(delay)).unwrap_err();
        assert!(matches!(error, AgentError::Command(_)));
        assert_eq!(delays, [50, 100, 150, 200, 250].map(Duration::from_millis));

        let mut attempts = 0;
        let mut delays = Vec::new();
        let value = retry_framebuffer_stream(
            || {
                attempts += 1;
                if attempts == 1 {
                    Err(AgentError::Command(
                        "framebuffer stream already has a desktop consumer".into(),
                    ))
                } else {
                    Ok(42)
                }
            },
            |delay| delays.push(delay),
        )
        .unwrap();
        assert_eq!(value, 42);
        assert_eq!(delays, [Duration::from_millis(50)]);

        let mut delayed = false;
        let error = retry_framebuffer_stream::<()>(
            || Err(AgentError::Protocol("bad hello".into())),
            |_| delayed = true,
        )
        .unwrap_err();
        assert!(matches!(error, AgentError::Protocol(_)));
        assert!(!delayed);
    }

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
    fn sd_list_protocol_cache_remembers_v2_and_unknown_command_fallback() {
        let cache = SdListProtocolCache::new();
        assert_eq!(cache.load(), SdListProtocol::Unknown);

        cache.remember(SdListProtocol::V2);
        assert_eq!(cache.load(), SdListProtocol::V2);

        assert!(is_unknown_command(&AgentError::Command(
            "unknown cmd".to_string()
        )));
        assert!(!is_unknown_command(&AgentError::Command(
            "permission denied".to_string()
        )));
        cache.remember(SdListProtocol::V1);
        assert_eq!(cache.load(), SdListProtocol::V1);
    }

    #[test]
    fn parse_device_telemetry_sample_extracts_ui_fields() {
        let sample = parse_device_telemetry_sample(
            r#"{
                "schema":"mister-magik-device-telemetry-v2",
                "seq":7,
                "presentation":{"schema":"mister-magik-presentation-telemetry-snapshot-v1","source":"fpga-owned-vblank-telemetry","available":true,"captured_monotonic_us":1000000,"owned_vblank_count":60,"presented_vblank_count":59,"repeated_vblank_count":1,"ownership_loss_count":0,"active_sequence":42,"magik_ownership":true,"pending":false,"lifetime_invariant_valid":true,"error":null},
                "cpu":{"combined_busy_pct":12.5,"cores":[{"id":0,"busy_pct":10.0},{"id":1,"busy_pct":15.0}]},
                "memory":{"total_kb":1000,"magik_kb":100,"main_kb":20,"other_used_kb":600,"available_kb":300,"magik_pct":10.0,"other_used_pct":60.0,"available_pct":30.0},
                "launcher":{"status_current":true,"idle":false,"rolling_fps":59.9,"preview_cache_state":"exact","frame_budget":{"budget_us":16667,"frames_total":120,"window_frames":60,"window_over_budget":2,"window_over_20ms":1,"window_over_33ms":0,"window_max_wall_us":21000,"max_wall_us":33000,"max_vsync_miss_streak":1,"window_prepare_us":100,"window_render_us":200,"window_custom_draw_us":300,"window_vsync_us":400,"window_present_us":500,"recent_frames":[{"frame":120,"wall_us":17000,"prepare_us":100,"render_us":200,"custom_draw_us":300,"vsync_us":400,"present_us":500,"cpu_prepare_us":10,"cpu_render_us":20,"cpu_custom_draw_us":30,"cpu_vsync_us":1,"cpu_present_us":5,"process_cpu_us":80,"vsync_source":"vsync","vsync_miss_streak":1}]}},
                "processes":{"mister-magik-fb":{"pids":[42],"rss_kb":100,"threads":7},"MiSTer_MagiK":{"pids":[9],"rss_kb":20,"threads":1}},
                "network":{"rx_bytes_per_sec":123,"tx_bytes_per_sec":456},
                "storage":{"available_bytes":1000,"total_bytes":2000,"available_pct":50.0,"device":"mmcblk0","activity_valid":true,"read_bytes_per_sec":12500000,"write_bytes_per_sec":2500000,"read_pct":25.0,"write_pct":10.0}
            }"#,
        )
        .expect("telemetry should parse");

        assert_eq!(sample.seq, 7);
        assert!(sample.presentation.available);
        assert_eq!(sample.presentation.repeated_vblank_count, Some(1));
        assert_eq!(sample.cores.len(), 2);
        assert_eq!(sample.cores[0].label, "CPU0");
        assert_eq!(sample.memory.magik_pct, 10.0);
        assert_eq!(sample.frame_budget.window_over_budget, 2);
        assert_eq!(sample.frame_budget.recent_frames.len(), 1);
        assert_eq!(sample.frame_budget.recent_frames[0].process_cpu_us, 80);
        assert_eq!(sample.launcher.fps, "59.9 fps");
        assert_eq!(sample.magik.pids, vec![42]);
        assert_eq!(sample.network.tx_bytes_per_sec, 456);
        assert_eq!(sample.storage.available_pct, 50.0);
        assert_eq!(sample.storage.device, "mmcblk0");
        assert!(sample.storage.activity_valid);
        assert_eq!(sample.storage.read_bytes_per_sec, 12_500_000);
        assert_eq!(sample.storage.write_pct, 10.0);
    }

    #[test]
    fn parse_device_telemetry_sample_defaults_missing_storage_activity() {
        let sample = parse_device_telemetry_sample(
            r#"{"schema":"mister-magik-device-telemetry-v2","seq":1,"presentation":{"schema":"mister-magik-presentation-telemetry-snapshot-v1","source":"fpga-owned-vblank-telemetry","available":false,"captured_monotonic_us":1,"error":"busy"},"storage":{"available_bytes":1000,"total_bytes":2000,"available_pct":50.0}}"#,
        )
        .expect("telemetry should parse");
        assert_eq!(sample.storage.device, "");
        assert!(!sample.storage.activity_valid);
        assert_eq!(sample.storage.read_bytes_per_sec, 0);
        assert_eq!(sample.storage.write_bytes_per_sec, 0);
        assert_eq!(sample.storage.read_pct, 0.0);
        assert_eq!(sample.storage.write_pct, 0.0);
    }

    #[test]
    fn parse_device_telemetry_sample_keeps_the_sampled_ui_thread_cpu() {
        let sample = parse_device_telemetry_sample(
            r#"{"schema":"mister-magik-device-telemetry-v2","presentation":{"schema":"mister-magik-presentation-telemetry-snapshot-v1","source":"fpga-owned-vblank-telemetry","available":false,"captured_monotonic_us":1,"error":"busy"},"launcher":{"ui_thread_cpu":1}}"#,
        )
        .expect("telemetry should parse");

        assert_eq!(sample.launcher.ui_thread_cpu, Some(1));
    }

    #[test]
    fn parse_device_telemetry_sample_rejects_legacy_streams() {
        let error = parse_device_telemetry_sample(
            r#"{"schema":"mister-magik-device-telemetry-v1"}"#,
        )
        .expect_err("legacy telemetry must not qualify");
        assert!(matches!(error, AgentError::Protocol(_)));
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
    fn local_token_path_defaults_to_worktree_build_directory() {
        let worktree_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("desktop crate should be nested under the worktree root");

        assert_eq!(
            local_token_path_from(None),
            worktree_root.join("build/mister-agent.token")
        );
    }

    #[test]
    fn read_token_prefers_env_then_configured_file_then_missing() {
        let token_path = env::temp_dir().join(format!("mister-agent-token-{}", std::process::id()));
        fs::write(&token_path, " file-token \n").expect("write token fixture");
        let token_path_text = token_path.to_str().expect("UTF-8 token fixture path");

        let (token, source) = read_token_from(None, Some(token_path_text));
        assert_eq!(token, "file-token");
        assert_eq!(source, TokenSource::LocalFile(token_path.clone()));

        let (token, source) = read_token_from(Some(" env-token "), Some(token_path_text));
        assert_eq!(token, "env-token");
        assert_eq!(source, TokenSource::Env);

        let missing = token_path.with_extension("missing");
        let (token, source) = read_token_from(
            Some("   "),
            Some(missing.to_str().expect("UTF-8 missing token path")),
        );
        assert_eq!(token, "");
        assert!(matches!(source, TokenSource::Missing(_)));

        let padded = format!("  {token_path_text}  ");
        let (token, source) = read_token_from(None, Some(&padded));
        assert_eq!(token, "file-token");
        assert_eq!(source, TokenSource::LocalFile(token_path.clone()));

        let (_, source) = read_token_from(None, Some("   "));
        let source_path = match source {
            TokenSource::LocalFile(path) | TokenSource::Missing(path) => path,
            TokenSource::Env => panic!("whitespace-only file override must use the default path"),
        };
        assert_eq!(source_path, local_token_path_from(None));

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
            "processes": {"MiSTer_MagiK": [10, 11], "mister-magik-fb": []},
            "scanout_slots": {"module_loaded": true, "device_ready": true}
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
        assert_eq!(snapshot.scanout_slots_summary, "Scanout slots ready");
        assert_eq!(snapshot.scanout_slots_detail, "module ready; device ready");
    }

    #[test]
    fn apply_magik_status_extracts_runtime_fields() {
        let mut snapshot = DashboardSnapshot::initial("host");
        let status = json!({
            "processes": {"MiSTer_MagiK": [10], "mister-magik-fb": [20]},
            "files": {
                "slint_status_current": true,
                "main_status": {"visible_owner": "fb0", "launcher_state": "LauncherActive", "scanout_slots_module_loaded": true, "scanout_slots_device_ready": true},
                "slint_status": {"screen": "Home", "scene": "launcher", "catalog_ready": true, "catalog_games": 5, "catalog_systems": 2, "input_pad_count": 1, "active_pad_name": "Pad"}
            }
        });
        apply_magik_status(&mut snapshot, &status);
        assert_eq!(snapshot.slint_status_freshness, "current");
        assert_eq!(snapshot.visible_owner, "fb0");
        assert_eq!(snapshot.launcher_state, "LauncherActive");
        assert_eq!(snapshot.catalog_summary, "ready; 5 games; 2 systems");
        assert_eq!(snapshot.scanout_slots_summary, "Scanout slots ready");
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
        assert_eq!(listing.round_trip_ms, 0);
        assert_eq!(listing.entries[0].kind, SdEntryKind::Directory);
        assert_eq!(listing.entries[1].kind, SdEntryKind::File);

        let v2 = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v2",
            "path": "/games",
            "elapsed_ms": 3,
            "entries": [{"name": "NES", "path": "/games/NES", "kind": "directory"}]
        }))
        .expect("lightweight v2 directory response should parse");
        assert_eq!(v2.entries.len(), 1);
        assert_eq!(v2.entries[0].name, "NES");

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
        assert!(
            fs::read(&capture.png_path)
                .expect("capture PNG should be written")
                .starts_with(b"\x89PNG\r\n\x1a\n")
        );
        let _ = fs::remove_file(capture.png_path);
    }

    #[test]
    fn parse_framebuffer_lz4_capture_expands_rgb565_pixels() {
        let raw = [0x00, 0xf8, 0xe0, 0x07];
        let payload = lz4_flex::compress_prepend_size(&raw);
        let capture = parse_framebuffer_capture_lz4(
            &json!({
                "schema": "mister-magik-framebuffer-raw-stream-v2",
                "source": "fb0",
                "capture_source": {"kind": "fb0"},
                "authoritative_scanout": false,
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
        assert_eq!(capture.raw_pixels, raw);
        assert_eq!(capture.raw_stride_bytes, 4);
        assert_eq!(capture.rgba_pixels, vec![255, 0, 0, 255, 0, 255, 0, 255]);
        assert_eq!(capture.timing.raw_read_us, 10);
        assert_eq!(capture.timing.lz4_encode_us, 20);
        assert_eq!(capture.timing.total_us, 30);
    }

    #[test]
    fn parse_framebuffer_lz4_capture_accepts_current_agent_contract() {
        let raw = [0x00, 0xf8];
        let payload = lz4_flex::compress_prepend_size(&raw);
        let capture = parse_framebuffer_capture_lz4(
            &json!({
                "schema": "mister-magik-framebuffer-raw-stream-v2",
                "source": "fpga-latched-scanout-slots",
                "capture_source": {
                    "kind": "fpga-latched-scanout-slots",
                    "active_base": "0x20000000",
                    "active_sequence": 7,
                    "region_index": 0,
                    "region_name": "menu"
                },
                "authoritative_scanout": true,
                "width": 1,
                "height": 1,
                "stride": 2,
                "bpp": 16,
                "format": "rgb565-le",
                "encoding": "lz4-block-size-prepended",
                "raw_bytes": 2,
                "payload_bytes": payload.len(),
                "content_nonzero_bytes": 1,
                "content_varied": true
            }),
            &payload,
        )
        .expect("current agent framebuffer contract should parse");

        assert_eq!(capture.raw_pixels, raw);
    }

    #[test]
    fn parse_framebuffer_lz4_capture_rejects_unknown_encoding() {
        let err = parse_framebuffer_capture_lz4(
            &json!({
                "schema": "mister-magik-framebuffer-raw-stream-v2",
                "source": "fb0",
                "capture_source": {"kind": "fb0"},
                "authoritative_scanout": false,
                "width": 1,
                "height": 1,
                "stride": 2,
                "bpp": 16,
                "format": "rgb565-le",
                "encoding": "zstd",
                "raw_bytes": 2,
                "payload_bytes": 2
            }),
            &[0, 0],
        )
        .expect_err("unsupported encoding should fail");

        assert!(matches!(err, AgentError::Protocol(message) if message.contains("unsupported")));
    }

    #[test]
    fn parse_framebuffer_lz4_capture_rejects_size_prefix_mismatch() {
        let payload = lz4_flex::compress_prepend_size(&[0u8; 8]);
        let err = parse_framebuffer_capture_lz4(
            &json!({
                "schema": "mister-magik-framebuffer-raw-stream-v2",
                "source": "fb0",
                "capture_source": {"kind": "fb0"},
                "authoritative_scanout": false,
                "width": 2,
                "height": 1,
                "stride": 4,
                "bpp": 16,
                "encoding": "lz4-block-size-prepended",
                "raw_bytes": 4,
                "payload_bytes": payload.len()
            }),
            &payload,
        )
        .expect_err("mismatched LZ4 size prefix should fail");

        assert!(err.to_string().contains("size prefix mismatch"));
    }

    #[test]
    fn parse_library_snapshot_lz4_verifies_payload() {
        let bytes = b"sqlite bytes";
        let payload = lz4_flex::block::compress(bytes);
        let snapshot = parse_library_database_snapshot_lz4(
            &json!({
                "schema": "mister-magik-library-db-snapshot-v1",
                "remote_path": "/media/fat/mister-magik-dev/library.sqlite3",
                "raw_bytes": bytes.len(),
                "payload_bytes": payload.len(),
                "encoding": "lz4-block",
                "checksum": fnv64_hex(bytes),
                "mtime_unix_ms": 1234
            }),
            &payload,
        )
        .expect("library snapshot should parse");

        assert_eq!(
            snapshot.remote_path,
            "/media/fat/mister-magik-dev/library.sqlite3"
        );
        assert_eq!(snapshot.bytes, bytes);
        assert_eq!(snapshot.raw_bytes, bytes.len() as u64);
        assert_eq!(snapshot.payload_bytes, payload.len() as u64);
        assert_eq!(snapshot.checksum, fnv64_hex(bytes));
        assert_eq!(snapshot.mtime_unix_ms, 1234);
    }

    #[test]
    fn parse_library_snapshot_accepts_development_agent_path() {
        let bytes = b"sqlite bytes";
        let payload = lz4_flex::block::compress(bytes);
        let snapshot = parse_library_database_snapshot_lz4(
            &json!({
                "schema": "mister-magik-library-db-snapshot-v1",
                "remote_path": "/media/fat/mister-magik-dev/library.sqlite3",
                "raw_bytes": bytes.len(),
                "payload_bytes": payload.len(),
                "encoding": "lz4-block",
                "checksum": fnv64_hex(bytes),
                "mtime_unix_ms": 1234
            }),
            &payload,
        )
        .expect("development Library snapshot should parse");

        assert_eq!(
            snapshot.remote_path,
            "/media/fat/mister-magik-dev/library.sqlite3"
        );
    }

    #[test]
    fn parse_library_snapshot_rejects_bad_checksum_and_path() {
        let bytes = b"sqlite bytes";
        let payload = lz4_flex::block::compress(bytes);
        let bad_checksum = parse_library_database_snapshot_lz4(
            &json!({
                "schema": "mister-magik-library-db-snapshot-v1",
                "remote_path": "/media/fat/mister-magik-dev/library.sqlite3",
                "raw_bytes": bytes.len(),
                "payload_bytes": payload.len(),
                "encoding": "lz4-block",
                "checksum": "0000000000000000"
            }),
            &payload,
        )
        .expect_err("checksum mismatch should fail");
        assert!(
            matches!(bad_checksum, AgentError::Protocol(message) if message.contains("checksum"))
        );

        let bad_path = parse_library_database_snapshot_lz4(
            &json!({
                "schema": "mister-magik-library-db-snapshot-v1",
                "remote_path": "/tmp/library.sqlite3",
                "raw_bytes": bytes.len(),
                "payload_bytes": payload.len(),
                "encoding": "lz4-block",
                "checksum": fnv64_hex(bytes)
            }),
            &payload,
        )
        .expect_err("path mismatch should fail");
        assert!(
            matches!(bad_path, AgentError::Protocol(message) if message.contains("allowlisted"))
        );

        let production_path = parse_library_database_snapshot_lz4(
            &json!({
                "schema": "mister-magik-library-db-snapshot-v1",
                "remote_path": "/media/fat/mister-magik/library.sqlite3",
                "raw_bytes": bytes.len(),
                "payload_bytes": payload.len(),
                "encoding": "lz4-block",
                "checksum": fnv64_hex(bytes)
            }),
            &payload,
        )
        .expect_err("production path should fail");
        assert!(
            matches!(production_path, AgentError::Protocol(message) if message.contains("allowlisted"))
        );
    }

    #[test]
    fn parse_library_snapshot_rejects_oversized_raw_length_before_decode() {
        let payload = lz4_flex::block::compress(b"small");
        let err = parse_library_database_snapshot_lz4(
            &json!({
                "schema": "mister-magik-library-db-snapshot-v1",
                "remote_path": "/media/fat/mister-magik-dev/library.sqlite3",
                "raw_bytes": MAX_AGENT_BINARY_PAYLOAD_BYTES + 1,
                "payload_bytes": payload.len(),
                "encoding": "lz4-block",
                "checksum": "unused"
            }),
            &payload,
        )
        .expect_err("oversized decoded snapshot should fail");

        assert!(err.to_string().contains("raw payload too large"));
    }

    #[test]
    fn framebuffer_stream_applies_keyframe_and_rect_delta() {
        let geometry = FrameGeometry {
            width: 3,
            height: 2,
            stride_pixels: 3,
        };
        let mut stream = FramebufferStreamState::default();
        let keyframe = (0_u8..12).collect::<Vec<_>>();
        let (header, payload) = encoded_stream_frame(
            FrameKind::Keyframe,
            1,
            geometry,
            FrameRect::full(geometry),
            &keyframe,
        );

        let frame = stream
            .apply_frame(header, &payload)
            .expect("keyframe should apply")
            .expect("keyframe should produce capture");

        assert_eq!(frame.capture.width, 3);
        assert_eq!(frame.capture.height, 2);
        assert_eq!(frame.kind, FrameKind::Keyframe);
        assert_eq!(frame.sequence, 1);
        assert_eq!(frame.timestamp_us, 123);
        assert_eq!(frame.geometry, geometry);
        assert_eq!(frame.rect, FrameRect::full(geometry));
        assert_eq!(stream.rgb565, keyframe);
        assert_eq!(stream.expected_sequence, Some(2));

        let rect = FrameRect {
            x: 1,
            y: 0,
            width: 1,
            height: 2,
        };
        let delta = [0xaa, 0xbb, 0xcc, 0xdd];
        let (header, payload) =
            encoded_stream_frame(FrameKind::RectDelta, 2, geometry, rect, &delta);

        let frame = stream
            .apply_frame(header, &payload)
            .expect("delta should apply")
            .expect("delta should produce capture");

        assert_eq!(frame.kind, FrameKind::RectDelta);
        assert_eq!(frame.sequence, 2);
        assert_eq!(frame.geometry, geometry);
        assert_eq!(frame.rect, rect);
        assert_eq!(frame.raw_bytes, delta.len() as u64);
        assert_eq!(frame.payload_bytes, payload.len() as u64);
        assert_eq!(
            stream.rgb565,
            vec![0, 1, 0xaa, 0xbb, 4, 5, 6, 7, 0xcc, 0xdd, 10, 11]
        );
        assert_eq!(stream.expected_sequence, Some(3));
    }

    #[test]
    fn framebuffer_stream_seed_capture_allows_first_rect_delta() {
        let mut stream = FramebufferStreamState::default();
        let seed = FramebufferCapture {
            png_path: PathBuf::new(),
            rgba_pixels: Vec::new(),
            raw_pixels: vec![0, 1, 2, 3, 4, 5, 6, 7],
            raw_stride_bytes: 4,
            width: 2,
            height: 2,
            bpp: 16,
            raw_bytes: 8,
            payload_bytes: 8,
            encoding: "lz4-block-size-prepended".to_string(),
            png_bytes: 0,
            png_hex_bytes: 0,
            timing: FramebufferCaptureTiming::default(),
        };
        stream
            .seed_from_capture(&seed)
            .expect("seed capture should apply");

        let geometry = FrameGeometry {
            width: 2,
            height: 2,
            stride_pixels: 2,
        };
        let rect = FrameRect {
            x: 1,
            y: 0,
            width: 1,
            height: 2,
        };
        let (header, payload) =
            encoded_stream_frame(FrameKind::RectDelta, 42, geometry, rect, &[8, 9, 10, 11]);
        let frame = stream
            .apply_frame(header, &payload)
            .expect("seeded delta should apply")
            .expect("seeded delta should produce capture");

        assert_eq!(frame.capture.width, 2);
        assert_eq!(frame.kind, FrameKind::RectDelta);
        assert_eq!(frame.sequence, 42);
        assert_eq!(frame.rect, rect);
        assert_eq!(stream.rgb565, vec![0, 1, 8, 9, 4, 5, 10, 11]);
        assert_eq!(stream.expected_sequence, Some(43));
    }

    #[test]
    fn framebuffer_stream_without_seed_waits_for_keyframe() {
        let geometry = FrameGeometry {
            width: 2,
            height: 1,
            stride_pixels: 2,
        };
        let mut stream = FramebufferStreamState::default();
        let (header, payload) = encoded_stream_frame(
            FrameKind::RectDelta,
            1,
            geometry,
            FrameRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            &[0xaa, 0xbb],
        );

        let capture = stream
            .apply_frame(header, &payload)
            .expect("unseeded delta should be ignored");

        assert!(capture.is_none());
        assert!(stream.awaiting_keyframe);
    }

    #[test]
    fn framebuffer_stream_waits_for_keyframe_after_sequence_gap() {
        let geometry = FrameGeometry {
            width: 2,
            height: 1,
            stride_pixels: 2,
        };
        let mut stream = FramebufferStreamState::default();
        let (header, payload) = encoded_stream_frame(
            FrameKind::Keyframe,
            10,
            geometry,
            FrameRect::full(geometry),
            &[0, 1, 2, 3],
        );
        stream
            .apply_frame(header, &payload)
            .expect("keyframe should apply")
            .expect("keyframe should produce capture");

        let (header, payload) = encoded_stream_frame(
            FrameKind::RectDelta,
            12,
            geometry,
            FrameRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            &[4, 5],
        );

        let capture = stream
            .apply_frame(header, &payload)
            .expect("sequence gap should be tolerated");

        assert!(capture.is_none());
        assert!(stream.awaiting_keyframe);

        let (header, payload) = encoded_stream_frame(
            FrameKind::RectDelta,
            13,
            geometry,
            FrameRect {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            },
            &[6, 7],
        );
        let capture = stream
            .apply_frame(header, &payload)
            .expect("delta while waiting should be tolerated");
        assert!(capture.is_none());

        let (header, payload) = encoded_stream_frame(
            FrameKind::Keyframe,
            14,
            geometry,
            FrameRect::full(geometry),
            &[8, 9, 10, 11],
        );
        let frame = stream
            .apply_frame(header, &payload)
            .expect("recovery keyframe should apply")
            .expect("recovery keyframe should produce capture");
        assert_eq!(frame.capture.width, 2);
        assert_eq!(frame.kind, FrameKind::Keyframe);
        assert_eq!(frame.sequence, 14);
        assert_eq!(frame.rect, FrameRect::full(geometry));
        assert!(!stream.awaiting_keyframe);
        assert_eq!(stream.expected_sequence, Some(15));
    }

    #[test]
    fn framebuffer_stream_reallocates_on_geometry_keyframe() {
        let first = FrameGeometry {
            width: 2,
            height: 1,
            stride_pixels: 2,
        };
        let second = FrameGeometry {
            width: 1,
            height: 2,
            stride_pixels: 1,
        };
        let mut stream = FramebufferStreamState::default();
        let (header, payload) = encoded_stream_frame(
            FrameKind::Keyframe,
            1,
            first,
            FrameRect::full(first),
            &[0, 1, 2, 3],
        );
        stream
            .apply_frame(header, &payload)
            .expect("first geometry should apply")
            .expect("first geometry should produce capture");

        let (header, payload) = encoded_stream_frame(
            FrameKind::Keyframe,
            20,
            second,
            FrameRect::full(second),
            &[4, 5, 6, 7],
        );
        let frame = stream
            .apply_frame(header, &payload)
            .expect("geometry keyframe should apply")
            .expect("geometry keyframe should produce capture");

        assert_eq!(frame.capture.width, 1);
        assert_eq!(frame.capture.height, 2);
        assert_eq!(frame.kind, FrameKind::Keyframe);
        assert_eq!(frame.geometry, second);
        assert_eq!(frame.rect, FrameRect::full(second));
        assert_eq!(stream.rgb565, vec![4, 5, 6, 7]);
        assert_eq!(stream.expected_sequence, Some(21));
    }

    #[test]
    fn framebuffer_stream_rejects_lz4_size_prefix_mismatch_before_decode() {
        let geometry = FrameGeometry {
            width: 2,
            height: 1,
            stride_pixels: 2,
        };
        let rect = FrameRect::full(geometry);
        let payload = lz4_flex::compress_prepend_size(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let header = FrameHeader {
            kind: FrameKind::Keyframe,
            flags: FLAG_LZ4_SIZE_PREPENDED,
            sequence: 1,
            timestamp_us: 0,
            geometry,
            rect,
            raw_bytes: 4,
            payload_bytes: payload.len() as u32,
        };

        let err = FramebufferStreamState::default()
            .apply_frame(header, &payload)
            .expect_err("mismatched LZ4 size prefix should fail");

        assert!(err.to_string().contains("size prefix mismatch"));
    }

    #[test]
    fn framebuffer_stream_rejects_unknown_encoding_flags() {
        let geometry = FrameGeometry {
            width: 2,
            height: 1,
            stride_pixels: 2,
        };
        let (mut header, payload) = encoded_stream_frame(
            FrameKind::Keyframe,
            1,
            geometry,
            FrameRect::full(geometry),
            &[1, 2, 3, 4],
        );
        header.flags |= 1 << 1;

        let err = FramebufferStreamState::default()
            .apply_frame(header, &payload)
            .expect_err("unknown encoding flags should fail closed");

        assert!(err.to_string().contains("unsupported flags 0x0003"));
    }

    #[test]
    fn binary_payload_len_is_bounded_before_allocation() {
        assert_eq!(
            binary_payload_len(&json!({"payload_bytes": 7, "raw_bytes": 99}))
                .expect("payload byte count"),
            7
        );
        assert_eq!(
            binary_payload_len(&json!({"raw_bytes": 11})).expect("raw byte fallback"),
            11
        );
        let err = binary_payload_len(&json!({
            "payload_bytes": MAX_AGENT_BINARY_PAYLOAD_BYTES + 1
        }))
        .expect_err("oversized binary payload should fail");
        assert!(err.to_string().contains("payload too large"));
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

    fn encoded_stream_frame(
        kind: FrameKind,
        sequence: u64,
        geometry: FrameGeometry,
        rect: FrameRect,
        raw: &[u8],
    ) -> (FrameHeader, Vec<u8>) {
        let payload = lz4_flex::compress_prepend_size(raw);
        (
            FrameHeader {
                kind,
                flags: FLAG_LZ4_SIZE_PREPENDED,
                sequence,
                timestamp_us: 123,
                geometry,
                rect,
                raw_bytes: raw.len() as u32,
                payload_bytes: payload.len() as u32,
            },
            payload,
        )
    }
}
