// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::{AgentError, AgentResult};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);
const JPEG_MEDIA_TYPE: &str = "image/jpeg";
const MOVIE_MEDIA_TYPE: &str = "video/quicktime";
const JPEG_WIDTH: u32 = 1920;
const JPEG_HEIGHT: u32 = 1080;
const MOVIE_MIN_SECONDS: u64 = 1;
const MOVIE_MAX_SECONDS: u64 = 60;
#[cfg(any(target_os = "macos", test))]
const MOVIE_DURATION_TOLERANCE_SECONDS: f64 = 0.5;
#[cfg(any(target_os = "macos", test))]
const MOVIE_MIN_DECODED_FRAMES_PER_SECOND: u64 = 5;
#[cfg(any(target_os = "macos", test))]
const SPATIAL_LUMA_SAMPLE_STEP: usize = 4;
#[cfg(any(target_os = "macos", test))]
const STRONG_ROW_DISCONTINUITY: u8 = 12;
const TEMPORAL_LUMA_GRID_COLUMNS: usize = 16;
const TEMPORAL_LUMA_GRID_ROWS: usize = 9;
const TEMPORAL_LUMA_GRID_LEN: usize = TEMPORAL_LUMA_GRID_COLUMNS * TEMPORAL_LUMA_GRID_ROWS;
const TEMPORAL_LUMA_STATIC_COLUMNS: usize = TEMPORAL_LUMA_GRID_COLUMNS / 2;
#[cfg(any(target_os = "macos", test))]
const TEMPORAL_LUMA_IGNORED_RIGHT_COLUMNS: usize = 32;
#[cfg(any(target_os = "macos", test))]
const TEMPORAL_LUMA_VIDEO_MINIMUM: u8 = 16;
#[cfg(any(target_os = "macos", test))]
const TEMPORAL_LUMA_VIDEO_RANGE: u16 = 219;
#[cfg(any(target_os = "macos", test))]
const TEMPORAL_LUMA_FULL_RANGE: u16 = 255;
pub(crate) const TEMPORAL_LUMA_GRID_ID: &str = "8x9-static-left-video-range-v2";
// Fixed-range normalization plus the static left half measured zero permille
// across 2,186 one-second comparisons from three known-good native movies.
// All 708 one-second comparisons from the preserved moving-corruption movie
// measured 2..=657 permille in the same region.
pub(crate) const TEMPORAL_LUMA_CORRUPTION_PERMILLE: u16 = 2;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CaptureArtifact {
    pub v: u8,
    pub event: &'static str,
    pub kind: &'static str,
    pub path: PathBuf,
    pub media_type: &'static str,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    pub capture_ms: u64,
}

impl CaptureArtifact {
    #[must_use]
    pub fn markdown_link(&self) -> String {
        let label = if self.kind == "usb_video_movie" {
            "USB Video recording"
        } else {
            "USB Video frame"
        };
        format!("[{label}](<{}>)", self.path.display())
    }
}

#[derive(Clone, Debug)]
struct EncodedFrame {
    jpeg: Vec<u8>,
    width: u32,
    height: u32,
    luma: Option<LumaAnalysis>,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq)]
struct MovieObservation {
    frames: u64,
    width: u32,
    height: u32,
    decoded_duration_seconds: f64,
}

#[cfg(any(target_os = "macos", test))]
fn validate_movie_observation(
    observation: MovieObservation,
    requested_duration: Duration,
) -> AgentResult<()> {
    if observation.width != JPEG_WIDTH || observation.height != JPEG_HEIGHT {
        return Err(classified(
            "camera_movie_validation_failed",
            format!(
                "AVFoundation decoded {}x{} video; expected {JPEG_WIDTH}x{JPEG_HEIGHT}",
                observation.width, observation.height
            ),
        ));
    }
    let requested_seconds = requested_duration.as_secs_f64();
    let minimum_seconds = (requested_seconds - MOVIE_DURATION_TOLERANCE_SECONDS).max(0.1);
    let maximum_seconds = requested_seconds + MOVIE_DURATION_TOLERANCE_SECONDS;
    if !observation.decoded_duration_seconds.is_finite()
        || !(minimum_seconds..=maximum_seconds).contains(&observation.decoded_duration_seconds)
    {
        return Err(classified(
            "camera_movie_validation_failed",
            format!(
                "AVFoundation decoded duration {:.3}s; expected {:.3}s..={:.3}s for a requested {:.3}s recording",
                observation.decoded_duration_seconds,
                minimum_seconds,
                maximum_seconds,
                requested_seconds
            ),
        ));
    }
    let minimum_frames = requested_duration
        .as_secs()
        .saturating_mul(MOVIE_MIN_DECODED_FRAMES_PER_SECOND)
        .max(1);
    if observation.frames < minimum_frames {
        return Err(classified(
            "camera_movie_validation_failed",
            format!(
                "AVFoundation decoded {} frames; expected at least {minimum_frames} for a requested {:.3}s recording",
                observation.frames, requested_seconds
            ),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureVisibility {
    Black,
    Corrupted,
    SignalLost,
    Visible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LumaAnalysis {
    minimum: u8,
    maximum: u8,
    mean: u8,
    strong_row_discontinuity_permille: u16,
    temporal_luma_grid: [u8; TEMPORAL_LUMA_GRID_LEN],
    visibility: CaptureVisibility,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AnalyzedCaptureArtifact {
    #[serde(flatten)]
    pub artifact: CaptureArtifact,
    pub visibility: CaptureVisibility,
    pub luma_minimum: u8,
    pub luma_maximum: u8,
    pub luma_mean: u8,
    pub strong_row_discontinuity_permille: u16,
    #[serde(skip)]
    temporal_luma_grid: [u8; TEMPORAL_LUMA_GRID_LEN],
}

impl AnalyzedCaptureArtifact {
    #[must_use]
    pub(crate) fn temporal_luma_delta_permille(&self, other: &Self) -> u16 {
        temporal_luma_delta_permille(&self.temporal_luma_grid, &other.temporal_luma_grid)
    }
}

trait CaptureBackend {
    fn capture(&self, timeout: Duration) -> AgentResult<EncodedFrame>;
}

struct NativeBackend;

impl CaptureBackend for NativeBackend {
    fn capture(&self, timeout: Duration) -> AgentResult<EncodedFrame> {
        native::capture(timeout)
    }
}

pub fn execute(output: Option<&Path>) -> AgentResult<CaptureArtifact> {
    let started = Instant::now();
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| classified("camera_clock_invalid", error.to_string()))?
        .as_millis();
    execute_with_backend(
        &NativeBackend,
        output,
        &std::env::temp_dir(),
        timestamp_ms,
        started,
    )
}

pub fn execute_analyzed(output: Option<&Path>) -> AgentResult<AnalyzedCaptureArtifact> {
    let started = Instant::now();
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| classified("camera_clock_invalid", error.to_string()))?
        .as_millis();
    let frame = native::capture_analyzed(CAPTURE_TIMEOUT)?;
    let luma = frame.luma.ok_or_else(|| {
        classified(
            "camera_analysis_unavailable",
            "USB Video capture did not include luma analysis",
        )
    })?;
    let artifact = store_frame(output, &std::env::temp_dir(), timestamp_ms, started, frame)?;
    Ok(AnalyzedCaptureArtifact {
        artifact,
        visibility: luma.visibility,
        luma_minimum: luma.minimum,
        luma_maximum: luma.maximum,
        luma_mean: luma.mean,
        strong_row_discontinuity_permille: luma.strong_row_discontinuity_permille,
        temporal_luma_grid: luma.temporal_luma_grid,
    })
}

pub fn execute_movie(output: Option<&Path>, seconds: u64) -> AgentResult<CaptureArtifact> {
    if !(MOVIE_MIN_SECONDS..=MOVIE_MAX_SECONDS).contains(&seconds) {
        return Err(classified(
            "camera_duration_invalid",
            format!(
                "USB Video movie duration must be {MOVIE_MIN_SECONDS}..={MOVIE_MAX_SECONDS} seconds"
            ),
        ));
    }
    let started = Instant::now();
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| classified("camera_clock_invalid", error.to_string()))?
        .as_millis();
    let destination = movie_destination(output, &std::env::temp_dir(), timestamp_ms)?;
    if let Err(error) = native::record(&destination, Duration::from_secs(seconds)) {
        let _ = fs::remove_file(&destination);
        return Err(error);
    }
    let bytes = fs::metadata(&destination)
        .map_err(|error| classified("camera_output_failed", error.to_string()))?
        .len()
        .try_into()
        .unwrap_or(usize::MAX);
    if bytes == 0 {
        let _ = fs::remove_file(&destination);
        return Err(classified(
            "camera_movie_validation_failed",
            "recorded USB Video movie file is empty after AVFoundation finalization",
        ));
    }
    Ok(CaptureArtifact {
        v: 1,
        event: "artifact",
        kind: "usb_video_movie",
        path: destination,
        media_type: MOVIE_MEDIA_TYPE,
        width: JPEG_WIDTH,
        height: JPEG_HEIGHT,
        bytes,
        capture_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
    })
}

fn movie_destination(
    output: Option<&Path>,
    temporary_root: &Path,
    timestamp_ms: u128,
) -> AgentResult<PathBuf> {
    if let Some(output) = output {
        return explicit_movie_destination(output);
    }
    let directory = temporary_capture_directory(temporary_root)?;
    for suffix in 1_u64.. {
        let suffix = if suffix == 1 {
            String::new()
        } else {
            format!("-{suffix}")
        };
        let path = directory.join(format!("mister-magik-usb-video-{timestamp_ms}{suffix}.mov"));
        if !path.exists() {
            return Ok(path);
        }
    }
    unreachable!("capture suffix space exhausted")
}

fn explicit_movie_destination(path: &Path) -> AgentResult<PathBuf> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if extension.as_deref() != Some("mov") {
        return Err(classified(
            "camera_output_invalid",
            format!("movie output must use a .mov extension: {}", path.display()),
        ));
    }
    let file_name = path.file_name().ok_or_else(|| {
        classified(
            "camera_output_invalid",
            format!("output has no file name: {}", path.display()),
        )
    })?;
    let parent = match path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent.to_path_buf(),
        None => std::env::current_dir()
            .map_err(|error| classified("camera_output_invalid", error.to_string()))?,
    };
    if !parent.is_dir() {
        return Err(classified(
            "camera_output_invalid",
            format!("output directory does not exist: {}", parent.display()),
        ));
    }
    let destination = fs::canonicalize(&parent)
        .map_err(|error| classified("camera_output_invalid", error.to_string()))?
        .join(file_name);
    if destination.exists() {
        return Err(classified(
            "camera_output_exists",
            format!("refusing to overwrite {}", destination.display()),
        ));
    }
    Ok(destination)
}

fn execute_with_backend(
    backend: &impl CaptureBackend,
    output: Option<&Path>,
    temporary_root: &Path,
    timestamp_ms: u128,
    started: Instant,
) -> AgentResult<CaptureArtifact> {
    if let Some(output) = output {
        explicit_destination(output)?;
    }
    let frame = backend.capture(CAPTURE_TIMEOUT)?;
    store_frame(output, temporary_root, timestamp_ms, started, frame)
}

fn store_frame(
    output: Option<&Path>,
    temporary_root: &Path,
    timestamp_ms: u128,
    started: Instant,
    frame: EncodedFrame,
) -> AgentResult<CaptureArtifact> {
    let destination = output.map(explicit_destination).transpose()?.map_or_else(
        || temporary_capture_directory(temporary_root).map(Destination::Temporary),
        |path| Ok(Destination::Explicit(path)),
    )?;
    if frame.width != JPEG_WIDTH || frame.height != JPEG_HEIGHT {
        return Err(classified(
            "camera_frame_invalid",
            format!(
                "USB Video returned {}x{}; expected {JPEG_WIDTH}x{JPEG_HEIGHT}",
                frame.width, frame.height
            ),
        ));
    }
    let path = write_capture(&destination, timestamp_ms, &frame.jpeg)?;
    Ok(CaptureArtifact {
        v: 1,
        event: "artifact",
        kind: "usb_video_frame",
        path,
        media_type: JPEG_MEDIA_TYPE,
        width: frame.width,
        height: frame.height,
        bytes: frame.jpeg.len(),
        capture_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
    })
}

enum Destination {
    Explicit(PathBuf),
    Temporary(PathBuf),
}

fn explicit_destination(path: &Path) -> AgentResult<PathBuf> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if !matches!(extension.as_deref(), Some("jpg" | "jpeg")) {
        return Err(classified(
            "camera_output_invalid",
            format!(
                "output must use a .jpg or .jpeg extension: {}",
                path.display()
            ),
        ));
    }
    let file_name = path.file_name().ok_or_else(|| {
        classified(
            "camera_output_invalid",
            format!("output has no file name: {}", path.display()),
        )
    })?;
    let parent = match path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent.to_path_buf(),
        None => std::env::current_dir()
            .map_err(|error| classified("camera_output_invalid", error.to_string()))?,
    };
    if !parent.is_dir() {
        return Err(classified(
            "camera_output_invalid",
            format!("output directory does not exist: {}", parent.display()),
        ));
    }
    let parent = fs::canonicalize(&parent)
        .map_err(|error| classified("camera_output_invalid", error.to_string()))?;
    let destination = parent.join(file_name);
    if destination.exists() {
        return Err(classified(
            "camera_output_exists",
            format!("refusing to overwrite {}", destination.display()),
        ));
    }
    Ok(destination)
}

fn temporary_capture_directory(root: &Path) -> AgentResult<PathBuf> {
    let directory = root.join("mister-magik").join("captures");
    fs::create_dir_all(&directory)
        .map_err(|error| classified("camera_output_failed", error.to_string()))?;
    fs::canonicalize(directory)
        .map_err(|error| classified("camera_output_failed", error.to_string()))
}

fn write_capture(
    destination: &Destination,
    timestamp_ms: u128,
    jpeg: &[u8],
) -> AgentResult<PathBuf> {
    match destination {
        Destination::Explicit(path) => write_new(path, jpeg),
        Destination::Temporary(directory) => {
            for suffix in 1_u64.. {
                let suffix = if suffix == 1 {
                    String::new()
                } else {
                    format!("-{suffix}")
                };
                let path =
                    directory.join(format!("mister-magik-usb-video-{timestamp_ms}{suffix}.jpg"));
                match write_new(&path, jpeg) {
                    Ok(path) => return Ok(path),
                    Err(AgentError::Classified {
                        code: "camera_output_exists",
                        ..
                    }) => {}
                    Err(error) => return Err(error),
                }
            }
            unreachable!("capture suffix space exhausted")
        }
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> AgentResult<PathBuf> {
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(classified(
                "camera_output_exists",
                format!("refusing to overwrite {}", path.display()),
            ));
        }
        Err(error) => return Err(classified("camera_output_failed", error.to_string())),
    };
    if let Err(error) = file.write_all(bytes) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(classified("camera_output_failed", error.to_string()));
    }
    Ok(path.to_path_buf())
}

fn classified(code: &'static str, detail: impl Into<String>) -> AgentError {
    AgentError::Classified {
        code,
        detail: detail.into(),
    }
}

#[must_use]
#[cfg(any(target_os = "macos", test))]
fn analyze_luma(
    luma: &[u8],
    width: usize,
    height: usize,
    row_bytes: usize,
) -> Option<LumaAnalysis> {
    let required = row_bytes.checked_mul(height)?;
    if width == 0 || height == 0 || row_bytes < width || luma.len() < required {
        return None;
    }
    let mut total = 0_u64;
    let mut samples = 0_u64;
    let mut minimum = u8::MAX;
    let mut maximum = u8::MIN;
    for y in (0..height).step_by(32) {
        for x in (0..width).step_by(32) {
            let sample = luma[y * row_bytes + x];
            total += u64::from(sample);
            samples += 1;
            minimum = minimum.min(sample);
            maximum = maximum.max(sample);
        }
    }
    if samples == 0 {
        return None;
    }
    let mean = u8::try_from(total / samples).unwrap_or(u8::MAX);
    // An all-zero plane is a missing/invalid camera sample. HDMI video-level
    // black is normally around luma 16 and is valid evidence worth retaining.
    if maximum <= 1 {
        return None;
    }
    let strong_row_discontinuity_permille =
        strong_row_discontinuity_permille(luma, width, height, row_bytes);
    let temporal_luma_grid = temporal_luma_grid(luma, width, height, row_bytes, minimum, maximum)?;
    let visibility = if is_capture_card_signal_loss(luma, width, height, row_bytes) {
        CaptureVisibility::SignalLost
    } else if mean > 24 || maximum.saturating_sub(minimum) > 12 {
        CaptureVisibility::Visible
    } else {
        CaptureVisibility::Black
    };
    Some(LumaAnalysis {
        minimum,
        maximum,
        mean,
        strong_row_discontinuity_permille,
        temporal_luma_grid,
        visibility,
    })
}

#[must_use]
#[cfg(any(target_os = "macos", test))]
fn temporal_luma_grid(
    luma: &[u8],
    width: usize,
    height: usize,
    row_bytes: usize,
    minimum: u8,
    maximum: u8,
) -> Option<[u8; TEMPORAL_LUMA_GRID_LEN]> {
    if width < TEMPORAL_LUMA_GRID_COLUMNS || height < TEMPORAL_LUMA_GRID_ROWS {
        return None;
    }
    let active_width = if width > TEMPORAL_LUMA_IGNORED_RIGHT_COLUMNS * 2 {
        width - TEMPORAL_LUMA_IGNORED_RIGHT_COLUMNS
    } else {
        width
    };
    let mut totals = [0_u64; TEMPORAL_LUMA_GRID_LEN];
    let mut samples = [0_u64; TEMPORAL_LUMA_GRID_LEN];
    for y in (0..height).step_by(SPATIAL_LUMA_SAMPLE_STEP) {
        let grid_y = y * TEMPORAL_LUMA_GRID_ROWS / height;
        for x in (0..active_width).step_by(SPATIAL_LUMA_SAMPLE_STEP) {
            let grid_x = x * TEMPORAL_LUMA_GRID_COLUMNS / active_width;
            let index = grid_y * TEMPORAL_LUMA_GRID_COLUMNS + grid_x;
            totals[index] += u64::from(canonical_temporal_luma(
                luma[y * row_bytes + x],
                minimum,
                maximum,
            ));
            samples[index] += 1;
        }
    }
    let mut grid = [0_u8; TEMPORAL_LUMA_GRID_LEN];
    for (index, sample_count) in samples.into_iter().enumerate() {
        if sample_count == 0 {
            return None;
        }
        grid[index] = u8::try_from(totals[index] / sample_count).unwrap_or(u8::MAX);
    }
    Some(grid)
}

#[must_use]
#[cfg(any(target_os = "macos", test))]
fn canonical_temporal_luma(value: u8, minimum: u8, maximum: u8) -> u8 {
    if minimum <= 1 && maximum >= 254 {
        let scaled = (u16::from(value) * TEMPORAL_LUMA_VIDEO_RANGE + TEMPORAL_LUMA_FULL_RANGE / 2)
            / TEMPORAL_LUMA_FULL_RANGE;
        u8::try_from(u16::from(TEMPORAL_LUMA_VIDEO_MINIMUM) + scaled).unwrap_or(u8::MAX)
    } else {
        value
    }
}

#[must_use]
fn temporal_luma_delta_permille(
    first: &[u8; TEMPORAL_LUMA_GRID_LEN],
    second: &[u8; TEMPORAL_LUMA_GRID_LEN],
) -> u16 {
    let delta = (0..TEMPORAL_LUMA_GRID_ROWS)
        .flat_map(|row| {
            let start = row * TEMPORAL_LUMA_GRID_COLUMNS;
            (start..start + TEMPORAL_LUMA_STATIC_COLUMNS)
                .map(|index| u64::from(first[index].abs_diff(second[index])))
        })
        .sum::<u64>();
    let maximum = 255_u64 * (TEMPORAL_LUMA_STATIC_COLUMNS * TEMPORAL_LUMA_GRID_ROWS) as u64;
    u16::try_from(delta * 1000 / maximum).unwrap_or(u16::MAX)
}

#[must_use]
#[cfg(any(target_os = "macos", test))]
fn strong_row_discontinuity_permille(
    luma: &[u8],
    width: usize,
    height: usize,
    row_bytes: usize,
) -> u16 {
    if height < 2 {
        return 0;
    }
    let samples_per_row = width.div_ceil(SPATIAL_LUMA_SAMPLE_STEP);
    let strong_total = usize::from(STRONG_ROW_DISCONTINUITY) * samples_per_row;
    let strong_rows = (1..height)
        .filter(|&y| {
            (0..width)
                .step_by(SPATIAL_LUMA_SAMPLE_STEP)
                .map(|x| {
                    usize::from(luma[y * row_bytes + x].abs_diff(luma[(y - 1) * row_bytes + x]))
                })
                .sum::<usize>()
                >= strong_total
        })
        .count();
    u16::try_from(strong_rows * 1000 / (height - 1)).unwrap_or(u16::MAX)
}

#[must_use]
#[cfg(any(target_os = "macos", test))]
fn is_capture_card_signal_loss(luma: &[u8], width: usize, height: usize, row_bytes: usize) -> bool {
    // This USB capture device substitutes eight full-height, video-range bars
    // when its HDMI input is absent. Treating those bars as ordinary nonblack
    // content turns a lost signal into a false physical-visibility pass.
    const BARS: [u8; 8] = [235, 210, 170, 145, 106, 81, 41, 16];
    const TOLERANCE: u8 = 6;

    if width < BARS.len() || height == 0 || row_bytes < width {
        return false;
    }
    let mut matched = 0_u64;
    let mut sampled = 0_u64;
    for (band, expected) in BARS.into_iter().enumerate() {
        for fraction in [1_usize, 2, 3] {
            let x = ((band * 4 + fraction) * width) / (BARS.len() * 4);
            for y in (0..height).step_by(32) {
                let sample = luma[y * row_bytes + x.min(width - 1)];
                sampled += 1;
                if sample.abs_diff(expected) <= TOLERANCE {
                    matched += 1;
                }
            }
        }
    }
    sampled > 0 && matched * 100 >= sampled * 95
}

#[cfg(target_os = "macos")]
mod native;

#[cfg(not(target_os = "macos"))]
mod native {
    use super::{EncodedFrame, classified};
    use crate::error::AgentResult;
    use std::path::Path;
    use std::time::Duration;

    pub(super) fn capture(_timeout: Duration) -> AgentResult<EncodedFrame> {
        Err(classified(
            "camera_unsupported",
            "USB Video capture is available only on macOS",
        ))
    }

    pub(super) fn capture_analyzed(_timeout: Duration) -> AgentResult<EncodedFrame> {
        Err(classified(
            "camera_unsupported",
            "USB Video still capture requires macOS",
        ))
    }

    pub(super) fn record(_output: &Path, _duration: Duration) -> AgentResult<()> {
        Err(classified(
            "camera_unsupported",
            "USB Video capture is available only on macOS",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FixtureBackend {
        calls: AtomicUsize,
        frame: AgentResult<EncodedFrame>,
    }

    impl CaptureBackend for FixtureBackend {
        fn capture(&self, _timeout: Duration) -> AgentResult<EncodedFrame> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.frame.clone()
        }
    }

    fn fixture_frame() -> EncodedFrame {
        EncodedFrame {
            jpeg: b"\xff\xd8fixture\xff\xd9".to_vec(),
            width: JPEG_WIDTH,
            height: JPEG_HEIGHT,
            luma: None,
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("agent-cli-capture-{label}-{}", std::process::id()))
    }

    #[test]
    fn luma_analysis_distinguishes_signal_black_and_visible_content() {
        assert_eq!(analyze_luma(&vec![0; 64 * 64], 64, 64, 64), None);
        assert_eq!(analyze_luma(&vec![16; 63 * 64], 64, 64, 64), None);
        assert_eq!(analyze_luma(&vec![16; 64 * 64], 65, 64, 64), None);
        assert_eq!(
            analyze_luma(&vec![16; 64 * 64], 64, 64, 64)
                .unwrap()
                .visibility,
            CaptureVisibility::Black
        );

        let mut padded = vec![0; 80 * 64];
        for y in 0..64 {
            padded[y * 80..y * 80 + 64].fill(16);
        }
        padded[32 * 80 + 32] = 48;
        assert_eq!(
            analyze_luma(&padded, 64, 64, 80).unwrap().visibility,
            CaptureVisibility::Visible
        );

        let bars = [235_u8, 210, 170, 145, 106, 81, 41, 16];
        let mut signal_lost = vec![0; 64 * 64];
        for y in 0..64 {
            for x in 0..64 {
                signal_lost[y * 64 + x] = bars[x * bars.len() / 64];
            }
        }
        assert_eq!(
            analyze_luma(&signal_lost, 64, 64, 64).unwrap().visibility,
            CaptureVisibility::SignalLost
        );

        let mut corrupted = vec![0; 64 * 64];
        for y in 0..64 {
            corrupted[y * 64..(y + 1) * 64].fill(if y % 2 == 0 { 16 } else { 64 });
        }
        corrupted[32 * 64 + 32] = 64;
        let analysis = analyze_luma(&corrupted, 64, 64, 64).unwrap();
        assert_eq!(analysis.visibility, CaptureVisibility::Visible);
        assert_eq!(analysis.strong_row_discontinuity_permille, 1000);
    }

    #[test]
    fn temporal_luma_detects_moving_corruption_but_ignores_preview_and_range() {
        let width = 192;
        let height = 108;
        let baseline_pixels = vec![32; width * height];
        let mut moving_band_pixels = baseline_pixels.clone();
        moving_band_pixels[36 * width..72 * width].fill(96);

        let baseline = analyze_luma(&baseline_pixels, width, height, width).unwrap();
        let moving_band = analyze_luma(&moving_band_pixels, width, height, width).unwrap();
        assert!(
            temporal_luma_delta_permille(
                &baseline.temporal_luma_grid,
                &moving_band.temporal_luma_grid
            ) >= TEMPORAL_LUMA_CORRUPTION_PERMILLE
        );
        assert_eq!(
            temporal_luma_delta_permille(
                &baseline.temporal_luma_grid,
                &baseline.temporal_luma_grid
            ),
            0
        );

        let mut animated_preview_pixels = vec![32; width * height];
        for row in animated_preview_pixels.chunks_exact_mut(width) {
            row[width / 2..].fill(160);
        }
        let animated_preview =
            analyze_luma(&animated_preview_pixels, width, height, width).unwrap();
        assert_eq!(
            temporal_luma_delta_permille(
                &baseline.temporal_luma_grid,
                &animated_preview.temporal_luma_grid
            ),
            0
        );

        let full_range_pixels = (0..width * height)
            .map(|index| {
                let x = index % width;
                let y = index / width;
                if (x / 32 + y / 32) % 2 == 0 { 0 } else { 255 }
            })
            .collect::<Vec<_>>();
        let video_range_pixels = full_range_pixels
            .iter()
            .map(|value| canonical_temporal_luma(*value, u8::MIN, u8::MAX))
            .collect::<Vec<_>>();
        let full_range = analyze_luma(&full_range_pixels, width, height, width).unwrap();
        let video_range = analyze_luma(&video_range_pixels, width, height, width).unwrap();
        assert_eq!(
            temporal_luma_delta_permille(
                &full_range.temporal_luma_grid,
                &video_range.temporal_luma_grid
            ),
            0
        );

        let mut right_edge_noise_pixels = baseline_pixels;
        for row in right_edge_noise_pixels.chunks_exact_mut(width) {
            row[width - TEMPORAL_LUMA_IGNORED_RIGHT_COLUMNS..].fill(235);
        }
        let right_edge_noise =
            analyze_luma(&right_edge_noise_pixels, width, height, width).unwrap();
        assert_eq!(
            temporal_luma_delta_permille(
                &baseline.temporal_luma_grid,
                &right_edge_noise.temporal_luma_grid
            ),
            0
        );
    }

    #[test]
    fn default_capture_uses_unique_temporary_jpeg_and_markdown_link() {
        let root = temp_root("default");
        fs::create_dir_all(&root).unwrap();
        let backend = FixtureBackend {
            calls: AtomicUsize::new(0),
            frame: Ok(fixture_frame()),
        };
        let first = execute_with_backend(&backend, None, &root, 1234, Instant::now()).unwrap();
        let second = execute_with_backend(&backend, None, &root, 1234, Instant::now()).unwrap();
        assert!(first.path.ends_with("mister-magik-usb-video-1234.jpg"));
        assert!(second.path.ends_with("mister-magik-usb-video-1234-2.jpg"));
        assert_eq!(fs::read(&first.path).unwrap(), b"\xff\xd8fixture\xff\xd9");
        assert_eq!(
            first.markdown_link(),
            format!("[USB Video frame](<{}>)", first.path.display())
        );
        let json = serde_json::to_value(&first).unwrap();
        assert_eq!(json["event"], "artifact");
        assert_eq!(json["kind"], "usb_video_frame");
        assert_eq!(json["media_type"], JPEG_MEDIA_TYPE);
        assert_eq!(json["width"], JPEG_WIDTH);
        assert_eq!(json["height"], JPEG_HEIGHT);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_capture_rejects_bad_extension_and_existing_output_before_capture() {
        let root = temp_root("explicit");
        fs::create_dir_all(&root).unwrap();
        let backend = FixtureBackend {
            calls: AtomicUsize::new(0),
            frame: Ok(fixture_frame()),
        };
        let png = root.join("frame.png");
        assert_eq!(
            execute_with_backend(&backend, Some(&png), &root, 1, Instant::now())
                .unwrap_err()
                .to_string(),
            format!(
                "camera_output_invalid: output must use a .jpg or .jpeg extension: {}",
                png.display()
            )
        );
        let jpg = root.join("frame.jpg");
        fs::write(&jpg, b"existing").unwrap();
        assert!(
            execute_with_backend(&backend, Some(&jpg), &root, 1, Instant::now())
                .unwrap_err()
                .to_string()
                .starts_with("camera_output_exists:")
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn movie_destination_is_bounded_unique_and_mov_only() {
        let root = temp_root("movie");
        fs::create_dir_all(&root).unwrap();
        let first = movie_destination(None, &root, 1234).unwrap();
        fs::write(&first, b"existing").unwrap();
        let second = movie_destination(None, &root, 1234).unwrap();
        assert!(first.ends_with("mister-magik/captures/mister-magik-usb-video-1234.mov"));
        assert!(second.ends_with("mister-magik/captures/mister-magik-usb-video-1234-2.mov"));
        assert!(
            explicit_movie_destination(&root.join("capture.mp4"))
                .unwrap_err()
                .to_string()
                .starts_with("camera_output_invalid:")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn movie_validation_requires_decodable_geometry_duration_and_frames() {
        let requested = Duration::from_secs(30);
        let valid = MovieObservation {
            frames: 728,
            width: JPEG_WIDTH,
            height: JPEG_HEIGHT,
            decoded_duration_seconds: 30.049,
        };
        validate_movie_observation(valid, requested).unwrap();

        for (observation, detail) in [
            (
                MovieObservation {
                    width: 1280,
                    ..valid
                },
                "decoded 1280x1080 video",
            ),
            (
                MovieObservation {
                    decoded_duration_seconds: 2.0,
                    ..valid
                },
                "decoded duration 2.000s",
            ),
            (
                MovieObservation {
                    frames: 149,
                    ..valid
                },
                "decoded 149 frames",
            ),
        ] {
            assert!(
                validate_movie_observation(observation, requested)
                    .unwrap_err()
                    .to_string()
                    .contains(detail)
            );
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_backend_is_classified() {
        assert_eq!(
            native::capture(Duration::from_secs(1))
                .unwrap_err()
                .to_string(),
            "camera_unsupported: USB Video capture is available only on macOS"
        );
    }
}
