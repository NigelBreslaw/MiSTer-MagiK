//! One authoritative RGB565 scanout snapshot; no preview or fb0 fallback.
#[cfg(any(target_os = "linux", test))]
use mister_magik_scanout_contract::{MAX_HEIGHT, MAX_STRIDE_BYTES, MAX_WIDTH, SLOT_CAPACITY_BYTES};
use serde::Serialize;

#[derive(Debug)]
pub(crate) struct CaptureError {
    pub code: &'static str,
    pub detail: String,
}

impl CaptureError {
    fn new(code: &'static str, detail: impl ToString) -> Self {
        Self {
            code,
            detail: detail.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Geometry {
    pub width: usize,
    pub height: usize,
    pub stride_bytes: usize,
}

#[cfg(any(target_os = "linux", test))]
impl Geometry {
    fn byte_len(&self) -> Result<usize, CaptureError> {
        if self.width == 0
            || self.width > MAX_WIDTH
            || self.height == 0
            || self.height > MAX_HEIGHT
            || self.stride_bytes < self.width * 2
            || self.stride_bytes > MAX_STRIDE_BYTES
            || !self.stride_bytes.is_multiple_of(2)
        {
            return Err(CaptureError::new(
                "capture-invalid-geometry",
                "invalid RGB565 scanout dimensions or stride",
            ));
        }
        self.stride_bytes
            .checked_mul(self.height)
            .filter(|len| *len <= SLOT_CAPACITY_BYTES)
            .ok_or_else(|| {
                CaptureError::new("capture-invalid-geometry", "scanout exceeds slot capacity")
            })
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Capture {
    #[serde(flatten)]
    pub geometry: Geometry,
    pub source: &'static str,
    pub pixel_format: &'static str,
    pub frame_sequence: u16,
    #[serde(skip)]
    pub pixels: Vec<u8>,
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn capture() -> Result<Capture, CaptureError> {
    Err(CaptureError::new(
        "capture-unsupported",
        "MiSTer scanout requires Linux",
    ))
}

#[cfg(target_os = "linux")]
pub(crate) fn capture() -> Result<Capture, CaptureError> {
    use mister_magik_mister_runtime::fpga::Fpga;
    use mister_magik_scanout_contract::{DEVICE, EXPECTED_LAYOUT, GET_LAYOUT, ScanoutSlotsLayout};
    use std::{fs::OpenOptions, io, os::fd::AsRawFd};

    let unavailable = |error: io::Error| CaptureError::new("capture-unavailable", error);
    let device = OpenOptions::new()
        .read(true)
        .write(true)
        .open(DEVICE)
        .map_err(unavailable)?;
    let mut layout = ScanoutSlotsLayout::default();
    // SAFETY: device is live and layout is the shared repr(C) kernel UAPI buffer.
    if unsafe { libc::ioctl(device.as_raw_fd(), GET_LAYOUT as libc::c_ulong, &mut layout) } != 0 {
        return Err(unavailable(io::Error::last_os_error()));
    }
    if layout != EXPECTED_LAYOUT {
        return Err(CaptureError::new(
            "capture-unsupported",
            "scanout slot layout is unsupported",
        ));
    }
    let mut fpga = Fpga::open().map_err(unavailable)?;
    let _guard = fpga.lock_latch_transaction().map_err(unavailable)?;
    fpga.read_magik_latched_fbuf_capabilities()
        .map_err(unavailable)?;
    let before = fpga.read_magik_latched_fbuf_status().map_err(unavailable)?;
    if !before.supported() || !before.active_enabled() || !before.magik_owned() {
        return Err(CaptureError::new(
            "capture-unsupported",
            "no active MagiK scanout",
        ));
    }
    let geometry = Geometry {
        width: before.active_width as usize,
        height: before.active_height as usize,
        stride_bytes: before.active_stride as usize,
    };
    let len = geometry.byte_len()?;
    let slot = layout
        .slots
        .iter()
        .find(|slot| slot.physical_address == before.active_base)
        .ok_or_else(|| {
            CaptureError::new(
                "capture-unavailable",
                "active buffer is outside scanout slots",
            )
        })?;
    // SAFETY: layout and geometry were validated against the shared contract.
    // The kernel's write-combined mapping requires a read/write descriptor.
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            layout.map_bytes as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            device.as_raw_fd(),
            slot.mmap_offset_bytes as libc::off_t,
        )
    };
    if mapped == libc::MAP_FAILED {
        return Err(unavailable(io::Error::last_os_error()));
    }
    if mapped.is_null() {
        // SAFETY: even an unusable null mapping must be released.
        unsafe {
            libc::munmap(mapped, layout.map_bytes as usize);
        }
        return Err(CaptureError::new(
            "capture-unavailable",
            "scanout mapping returned null",
        ));
    }
    // SAFETY: the mapping spans at least len bytes and remains live until the copy finishes.
    let pixels = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), len).to_vec() };
    // SAFETY: unmap exactly the mapping returned above; pixels now owns its copy.
    unsafe {
        libc::munmap(mapped, layout.map_bytes as usize);
    }
    let after = fpga.read_magik_latched_fbuf_status().map_err(unavailable)?;
    if before.active_base != after.active_base
        || before.active_sequence != after.active_sequence
        || before.flip_count != after.flip_count
        || before.active_route_epoch != after.active_route_epoch
    {
        return Err(CaptureError::new(
            "capture-frame-changed",
            "scanout changed during capture; request another screenshot",
        ));
    }
    Ok(Capture {
        geometry,
        source: "fpga-latched-scanout-slots",
        pixel_format: "rgb565-le",
        frame_sequence: before.active_sequence,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_dimensions_and_padded_stride_before_reading() {
        assert_eq!(
            Geometry {
                width: 3,
                height: 2,
                stride_bytes: 8
            }
            .byte_len()
            .unwrap(),
            16
        );
        for (width, height, stride_bytes) in [
            (0, 1, 2),
            (1, 0, 2),
            (MAX_WIDTH + 1, 1, 2),
            (1, MAX_HEIGHT + 1, 2),
            (3, 1, 4),
            (1, 1, 3),
            (1, 1, MAX_STRIDE_BYTES + 2),
        ] {
            assert_eq!(
                Geometry {
                    width,
                    height,
                    stride_bytes
                }
                .byte_len()
                .unwrap_err()
                .code,
                "capture-invalid-geometry"
            );
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unavailable_capture_never_returns_a_preview() {
        assert_eq!(capture().unwrap_err().code, "capture-unsupported");
    }
}
