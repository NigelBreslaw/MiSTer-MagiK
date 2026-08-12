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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureVisibility {
    Black,
    Visible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LumaAnalysis {
    minimum: u8,
    maximum: u8,
    mean: u8,
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
    native::record(&destination, Duration::from_secs(seconds))?;
    let bytes = fs::metadata(&destination)
        .map_err(|error| classified("camera_output_failed", error.to_string()))?
        .len()
        .try_into()
        .unwrap_or(usize::MAX);
    if bytes == 0 {
        let _ = fs::remove_file(&destination);
        return Err(classified(
            "camera_recording_failed",
            "USB Video movie is empty",
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
    let visibility = if mean > 24 || maximum.saturating_sub(minimum) > 12 {
        CaptureVisibility::Visible
    } else {
        CaptureVisibility::Black
    };
    Some(LumaAnalysis {
        minimum,
        maximum,
        mean,
        visibility,
    })
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
