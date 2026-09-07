// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::app_state::{
    ConnectionOutcome, DashboardSnapshot, catalog_summary, input_summary, process_summary,
    screen_summary, string_at, uptime_label,
};
pub use crate::native_client::AgentError;
use crate::native_client::{
    Client,
    subscription::{Control, Subscription},
    wire,
};
use crate::sd_card::{
    SdDirectoryListing, SdEntry, SdEntryKind, SdItemDetail, SdMetadataRow, item_name,
};
use mister_magik_framebuffer_stream::{
    FLAG_LZ4_SIZE_PREPENDED, FrameGeometry, FrameHeader, FrameKind, FrameRect,
    MAX_FRAME_SURFACE_BYTES, read_frame,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::{env, fs};

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

pub struct FramebufferStream {
    session: Subscription,
    state: FramebufferStreamState,
}
pub struct DeviceTelemetryStream {
    session: Subscription,
}
pub type FramebufferStreamControl = Control;
pub type DeviceTelemetryStreamControl = Control;

struct FramebufferStreamState {
    rgb565: Vec<u8>,
    geometry: Option<FrameGeometry>,
    expected_sequence: Option<u64>,
    awaiting_keyframe: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceTelemetrySample {
    pub seq: u64,
    pub unavailable: Vec<String>,
    pub combined_cpu_pct: f64,
    pub cores: Vec<CpuCoreTelemetry>,
    pub memory: MemoryTelemetry,
    pub frame_budget: FrameBudgetTelemetry,
    pub launcher: LauncherTelemetry,
    pub magik: ProcessTelemetry,
    pub main: ProcessTelemetry,
    pub network: NetworkTelemetry,
    pub storage: StorageTelemetry,
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

pub fn fetch_dashboard(host: &str) -> DashboardSnapshot {
    let mut snapshot = DashboardSnapshot::initial(host);
    snapshot.token_source = "Shared native device credentials".into();
    let client = match Client::open(host) {
        Ok(client) => client,
        Err(error) => {
            snapshot.connection_state = match error {
                AgentError::Unauthorized => ConnectionOutcome::Unauthenticated,
                AgentError::Unreachable(_) => ConnectionOutcome::Unreachable,
                _ => ConnectionOutcome::ProtocolError,
            }
            .label()
            .into();
            snapshot.last_error = error.to_string();
            return snapshot;
        }
    };
    snapshot.host = client.address.clone();
    snapshot.connection_state = ConnectionOutcome::Ready.label().into();
    snapshot.agent_status = format!("Native service at {}", client.address);
    match client.request("status", json!({})) {
        Ok(status) => {
            snapshot.agent_version = status["identity"].as_str().unwrap_or("native").into();
            snapshot.agent_uptime = uptime_label(status["uptime_ms"].as_u64());
        }
        Err(err) => snapshot.last_error = err.to_string(),
    }

    match client.request("dashboard-status", json!({})) {
        Ok(mut status) => {
            status["network"]["ip"] = json!(client.address);
            if let Some(processes) = status["processes"].as_object_mut() {
                for process in processes.values_mut() {
                    *process = process["pids"].clone();
                }
            }
            apply_agent_status(&mut snapshot, &status);
            apply_magik_status(&mut snapshot, &status);
        }
        Err(err) => snapshot.last_error = err.to_string(),
    }

    snapshot
}

pub fn fetch_sd_directory(
    host: &str,
    path: &str,
    show_hidden: bool,
) -> Result<SdDirectoryListing, AgentError> {
    let client = Client::open(host)?;
    let args = json!({ "path": path, "show_hidden": show_hidden });
    let started = Instant::now();
    let value = client.request("sd-list", args)?;
    let mut listing = parse_sd_directory(&value)?;
    listing.round_trip_ms = started.elapsed().as_millis() as u64;
    Ok(listing)
}

pub fn fetch_sd_item_detail(host: &str, path: &str) -> Result<SdItemDetail, AgentError> {
    let client = Client::open(host)?;
    let stat = client.request("sd-stat", json!({ "path": path }))?;
    let mut detail = parse_sd_item_detail(&stat)?;

    if detail.has_image {
        match client.binary("sd-preview", json!({ "path": path })) {
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
        match client.request("sd-mra", json!({ "path": path })) {
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
    let client = Client::open(host)?;
    let (value, payload) = client.binary("capture-framebuffer", json!({}))?;
    parse_native_capture(&value, payload)
}

pub fn connect_framebuffer_stream(host: &str) -> Result<FramebufferStream, AgentError> {
    connect_framebuffer_stream_seeded(host, None)
}

pub fn connect_device_telemetry_stream(host: &str) -> Result<DeviceTelemetryStream, AgentError> {
    Ok(DeviceTelemetryStream {
        session: Subscription::open(host, "telemetry-stream")?,
    })
}

pub fn connect_framebuffer_stream_seeded(
    host: &str,
    seed: Option<&FramebufferCapture>,
) -> Result<FramebufferStream, AgentError> {
    let session = Subscription::open(host, "framebuffer-stream")?;
    let mut state = FramebufferStreamState::default();
    if let Some(seed) = seed {
        state.seed_from_capture(seed)?;
    }
    Ok(FramebufferStream { session, state })
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
    let client = Client::open(host)?;
    let (_, mut reader) = client.subscribe("framebuffer-stream")?;
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
        Ok(self.session.control.clone())
    }

    pub fn next_capture(&mut self) -> Result<FramebufferCapture, AgentError> {
        self.next_frame().map(|frame| frame.capture)
    }

    pub fn next_frame(&mut self) -> Result<FramebufferStreamFrame, AgentError> {
        loop {
            let read_started = Instant::now();
            let (header, payload) = match read_frame(&mut self.session.reader) {
                Ok(frame) => frame,
                Err(error) => {
                    let error = AgentError::from(error);
                    if self.session.reconnect(&error)? {
                        self.state = FramebufferStreamState::default();
                        continue;
                    }
                    return Err(error);
                }
            };
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
        Ok(self.session.control.clone())
    }

    pub fn next_sample(&mut self) -> Result<DeviceTelemetrySample, AgentError> {
        loop {
            match wire::read(
                &mut self.session.reader,
                &self.session.request_id,
                "telemetry-sample",
            ) {
                Ok((_, body)) => {
                    return parse_device_telemetry_sample(
                        std::str::from_utf8(&body)
                            .map_err(|e| AgentError::Protocol(e.to_string()))?,
                    );
                }
                Err(error) => {
                    if !self.session.reconnect(&error)? {
                        return Err(error);
                    }
                }
            }
        }
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

fn parse_device_telemetry_sample(line: &str) -> Result<DeviceTelemetrySample, AgentError> {
    let value: Value = serde_json::from_str(line.trim())
        .map_err(|err| AgentError::Protocol(format!("invalid telemetry JSON: {err}")))?;
    let frame = value
        .pointer("/launcher/frame_budget")
        .unwrap_or(&Value::Null);
    Ok(DeviceTelemetrySample {
        seq: u64_at(&value, "/seq"),
        unavailable: ["cpu", "memory", "network", "storage", "processes"]
            .into_iter()
            .filter(|key| value[*key].is_null())
            .map(str::to_string)
            .collect(),
        combined_cpu_pct: f64_at(&value, "/cpu/combined_busy_pct"),
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
        Some("mister-magik-sd-list-dir-v2")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_device_telemetry_sample_extracts_ui_fields() {
        let sample = parse_device_telemetry_sample(
            r#"{"seq":7,"cpu":{"combined_busy_pct":12.5,"cores":[{"id":0,"busy_pct":10.0},{"id":1,"busy_pct":15.0}]},"memory":{"total_kb":1000,"magik_kb":100,"main_kb":20,"other_used_kb":600,"available_kb":300,"magik_pct":10.0,"other_used_pct":60.0,"available_pct":30.0},"launcher":{"status_current":true,"idle":false,"rolling_fps":59.9,"preview_cache_state":"exact","frame_budget":{"budget_us":16667,"frames_total":120,"window_frames":60,"window_prepare_us":100,"window_render_us":200,"window_custom_draw_us":300,"window_vsync_us":400,"window_present_us":500,"recent_frames":[{"frame":120,"wall_us":17000,"prepare_us":100,"render_us":200,"custom_draw_us":300,"vsync_us":400,"present_us":500,"cpu_prepare_us":10,"cpu_render_us":20,"cpu_custom_draw_us":30,"cpu_vsync_us":1,"cpu_present_us":5,"process_cpu_us":80,"vsync_source":"vsync"}]}},"processes":{"mister-magik-fb":{"pids":[42],"rss_kb":100,"threads":7},"MiSTer_MagiK":{"pids":[9],"rss_kb":20,"threads":1}},"network":{"rx_bytes_per_sec":123,"tx_bytes_per_sec":456},"storage":{"available_bytes":1000,"total_bytes":2000,"available_pct":50.0,"device":"mmcblk0","activity_valid":true,"read_bytes_per_sec":12500000,"write_bytes_per_sec":2500000,"read_pct":25.0,"write_pct":10.0}}"#,
        )
        .expect("telemetry should parse");

        assert_eq!(sample.seq, 7);
        assert_eq!(sample.cores.len(), 2);
        assert_eq!(sample.cores[0].label, "CPU0");
        assert_eq!(sample.memory.magik_pct, 10.0);
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
            r#"{"seq":1,"storage":{"available_bytes":1000,"total_bytes":2000,"available_pct":50.0}}"#,
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
        let sample = parse_device_telemetry_sample(r#"{"launcher":{"ui_thread_cpu":1}}"#)
            .expect("telemetry should parse");

        assert_eq!(sample.launcher.ui_thread_cpu, Some(1));
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
            "schema": "mister-magik-sd-list-dir-v2",
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
    fn parse_sd_directory_reports_missing_container_fields() {
        let missing_path = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v2",
            "entries": []
        }))
        .expect_err("missing path should fail");
        assert!(
            matches!(missing_path, AgentError::Protocol(message) if message == "missing sd_list_dir path")
        );

        let missing_entries = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v2",
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
            "schema": "mister-magik-sd-list-dir-v2",
            "path": "/",
            "entries": [{"path": "/bad", "kind": "file"}]
        }))
        .expect_err("missing entry name should fail");
        assert!(
            matches!(missing_name, AgentError::Protocol(message) if message == "missing sd entry name")
        );

        let missing_path = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v2",
            "path": "/",
            "entries": [{"name": "bad", "kind": "file"}]
        }))
        .expect_err("missing entry path should fail");
        assert!(
            matches!(missing_path, AgentError::Protocol(message) if message == "missing sd entry path")
        );

        let missing_kind = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v2",
            "path": "/",
            "entries": [{"name": "bad", "path": "/bad"}]
        }))
        .expect_err("missing entry kind should fail");
        assert!(
            matches!(missing_kind, AgentError::Protocol(message) if message == "missing sd entry kind")
        );

        let unsupported_kind = parse_sd_directory(&json!({
            "schema": "mister-magik-sd-list-dir-v2",
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
        assert_eq!(
            AgentError::Unauthorized.to_string(),
            "native service rejected authentication"
        );
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

fn parse_native_capture(value: &Value, raw: Vec<u8>) -> Result<FramebufferCapture, AgentError> {
    let width = u64_at(value, "/width");
    let height = u64_at(value, "/height");
    let stride = u64_at(value, "/stride_bytes");
    if value["source"] != "fpga-latched-scanout-slots"
        || value["pixel_format"] != "rgb565-le"
        || value["frame_sequence"].as_u64().is_none()
        || width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || !stride.is_multiple_of(2)
        || stride.checked_mul(height) != Some(raw.len() as u64)
        || raw.len() > MAX_FRAME_SURFACE_BYTES
    {
        return Err(AgentError::Protocol(
            "invalid authoritative framebuffer geometry/source".into(),
        ));
    }
    let rgba_pixels = framebuffer_raw_to_rgba(&raw, width, height, stride, 16)?;
    Ok(FramebufferCapture {
        png_path: PathBuf::new(),
        rgba_pixels,
        raw_bytes: raw.len() as u64,
        payload_bytes: raw.len() as u64,
        raw_pixels: raw,
        raw_stride_bytes: stride,
        width,
        height,
        bpp: 16,
        encoding: format!(
            "fpga-latched-scanout-slots; frame {}",
            value["frame_sequence"]
        ),
        png_bytes: 0,
        png_hex_bytes: 0,
        timing: FramebufferCaptureTiming::default(),
    })
}
