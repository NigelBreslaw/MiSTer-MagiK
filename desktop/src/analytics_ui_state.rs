use crate::agent_client::FramebufferCapture;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CaptureUiState {
    pub loading: bool,
    pub has_image: bool,
    pub can_save_image: bool,
    pub width: u64,
    pub height: u64,
    pub status: String,
    pub last_error: String,
    pub clear_dirty_rects: bool,
}

pub(crate) fn loading_capture_state() -> CaptureUiState {
    CaptureUiState {
        loading: true,
        has_image: false,
        can_save_image: false,
        width: 0,
        height: 0,
        status: "Capturing framebuffer stream...".into(),
        last_error: String::new(),
        clear_dirty_rects: false,
    }
}

pub(crate) fn capture_result_state(result: Result<&FramebufferCapture, &str>) -> CaptureUiState {
    match result {
        Ok(capture) => CaptureUiState {
            loading: false,
            has_image: true,
            can_save_image: true,
            width: capture.width,
            height: capture.height,
            status: format!(
                "Captured {}x{} {}bpp framebuffer ({} payload; {} raw; {}).",
                capture.width,
                capture.height,
                capture.bpp,
                format_byte_size(capture.payload_bytes),
                format_byte_size(capture.raw_bytes),
                capture.encoding
            ),
            last_error: String::new(),
            clear_dirty_rects: true,
        },
        Err(error) => CaptureUiState {
            loading: false,
            has_image: false,
            can_save_image: false,
            width: 0,
            height: 0,
            status: "Framebuffer capture failed.".into(),
            last_error: error.into(),
            clear_dirty_rects: false,
        },
    }
}

pub(crate) fn stream_capture_state(
    capture: &FramebufferCapture,
    geometry_changed: bool,
) -> Option<CaptureUiState> {
    geometry_changed.then(|| CaptureUiState {
        loading: false,
        has_image: true,
        can_save_image: true,
        width: capture.width,
        height: capture.height,
        status: String::new(),
        last_error: String::new(),
        clear_dirty_rects: false,
    })
}

pub(crate) fn generation_is_current(current: u64, candidate: u64) -> bool {
    current == candidate
}

pub(crate) fn format_byte_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / MB)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_client::FramebufferCaptureTiming;
    use std::path::PathBuf;

    fn capture() -> FramebufferCapture {
        FramebufferCapture {
            png_path: PathBuf::new(),
            rgba_pixels: vec![0; 8],
            raw_pixels: Vec::new(),
            raw_stride_bytes: 0,
            width: 2,
            height: 1,
            bpp: 16,
            raw_bytes: 4096,
            payload_bytes: 1024,
            encoding: "fixture/lz4".into(),
            png_bytes: 0,
            png_hex_bytes: 0,
            timing: FramebufferCaptureTiming::default(),
        }
    }

    #[test]
    fn capture_lifecycle_reduces_loading_success_and_failure() {
        let loading = loading_capture_state();
        assert!(loading.loading);
        assert!(!loading.has_image);

        let capture = capture();
        let success = capture_result_state(Ok(&capture));
        assert!(!success.loading);
        assert!(success.has_image && success.can_save_image);
        assert_eq!((success.width, success.height), (2, 1));
        assert_eq!(
            success.status,
            "Captured 2x1 16bpp framebuffer (1 KB payload; 4 KB raw; fixture/lz4)."
        );
        assert!(success.clear_dirty_rects);

        let failure = capture_result_state(Err("offline"));
        assert!(!failure.loading);
        assert!(!failure.has_image);
        assert_eq!(failure.status, "Framebuffer capture failed.");
        assert_eq!(failure.last_error, "offline");
    }

    #[test]
    fn stream_updates_geometry_only_when_it_changes() {
        let capture = capture();
        assert!(stream_capture_state(&capture, false).is_none());
        let state = stream_capture_state(&capture, true).unwrap();
        assert_eq!((state.width, state.height), (2, 1));
        assert!(state.has_image);
    }

    #[test]
    fn stale_generation_is_rejected() {
        assert!(generation_is_current(7, 7));
        assert!(!generation_is_current(8, 7));
    }
}
