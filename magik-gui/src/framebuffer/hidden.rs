//! Experimental access to Main_MiSTer's hidden HPS framebuffer slots.
//!
//! `/dev/fb0` exposes only buffer 0 with a fast write-combined mapping. Main can
//! route hidden buffers 1 and 2, but Rust has to reach them through `/dev/mem`.
//! This module keeps that slow-path experiment explicit and geometry-checked.

use crate::framebuffer::format::rgb565_stride_bytes;
use slint::platform::software_renderer::Rgb565Pixel;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;

pub const MISTER_FB_SIZE_PIXELS: usize = 1920 * 1080;
pub const MISTER_FB_SLOT_BYTES: usize = MISTER_FB_SIZE_PIXELS * 4;
pub const MISTER_FB_PHYS_BASE: usize = 0x2000_0000 + (32 * 1024 * 1024);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HiddenRgb565BufferIndex(u8);

impl HiddenRgb565BufferIndex {
    pub fn new(index: u8) -> Result<Self, HiddenFramebufferError> {
        match index {
            1 | 2 => Ok(Self(index)),
            _ => Err(HiddenFramebufferError::InvalidBufferIndex { index }),
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub fn physical_address(self) -> Result<usize, HiddenFramebufferError> {
        hidden_buffer_physical_address(self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HiddenFramebufferError {
    InvalidBufferIndex {
        index: u8,
    },
    InvalidGeometry {
        width: usize,
        height: usize,
    },
    InvalidStride {
        stride_bytes: usize,
        min_stride_bytes: usize,
    },
    AddressOverflow,
    MapTooLarge {
        map_len: usize,
        slot_bytes: usize,
    },
    SourceTooShort {
        needed: usize,
        actual: usize,
    },
}

impl std::fmt::Display for HiddenFramebufferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBufferIndex { index } => {
                write!(f, "hidden framebuffer index must be 1 or 2, got {index}")
            }
            Self::InvalidGeometry { width, height } => {
                write!(f, "invalid hidden framebuffer geometry {width}x{height}")
            }
            Self::InvalidStride {
                stride_bytes,
                min_stride_bytes,
            } => write!(
                f,
                "hidden framebuffer stride {stride_bytes} is smaller than {min_stride_bytes}"
            ),
            Self::AddressOverflow => write!(f, "hidden framebuffer address overflow"),
            Self::MapTooLarge {
                map_len,
                slot_bytes,
            } => write!(
                f,
                "hidden framebuffer map length {map_len} exceeds slot size {slot_bytes}"
            ),
            Self::SourceTooShort { needed, actual } => {
                write!(
                    f,
                    "hidden framebuffer source has {actual} pixels, need {needed}"
                )
            }
        }
    }
}

impl std::error::Error for HiddenFramebufferError {}

pub struct HiddenRgb565Framebuffer {
    mem: *mut u8,
    map_len: usize,
    width: usize,
    height: usize,
    stride_pixels: usize,
    _mem_file: File,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HiddenMapSpec {
    phys_addr: usize,
    map_len: usize,
    stride_pixels: usize,
}

impl HiddenRgb565Framebuffer {
    pub fn open(
        index: HiddenRgb565BufferIndex,
        width: usize,
        height: usize,
        stride_bytes: usize,
    ) -> io::Result<Self> {
        let spec = validate_hidden_map(index, width, height, stride_bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
        let mem_file = OpenOptions::new().read(true).write(true).open("/dev/mem")?;
        let mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                spec.map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                mem_file.as_raw_fd(),
                spec.phys_addr as libc::off_t,
            )
        };
        if mem == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        if mem.is_null() {
            unsafe {
                libc::munmap(mem, spec.map_len);
            }
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "hidden framebuffer mmap returned a null address",
            ));
        }
        Ok(Self {
            mem: mem.cast::<u8>(),
            map_len: spec.map_len,
            width,
            height,
            stride_pixels: spec.stride_pixels,
            _mem_file: mem_file,
        })
    }

    pub fn copy_full_frame(
        &mut self,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
    ) -> Result<usize, HiddenFramebufferError> {
        let needed = src_stride_pixels
            .checked_mul(self.height)
            .ok_or(HiddenFramebufferError::AddressOverflow)?;
        if src.len() < needed {
            return Err(HiddenFramebufferError::SourceTooShort {
                needed,
                actual: src.len(),
            });
        }
        let width = self.width;
        let height = self.height;
        let stride_pixels = self.stride_pixels;
        let dst = self.buffer_mut();
        for y in 0..height {
            let src_start = y * src_stride_pixels;
            let dst_start = y * stride_pixels;
            dst[dst_start..dst_start + width].copy_from_slice(&src[src_start..src_start + width]);
        }
        Ok(stride_pixels * height * std::mem::size_of::<Rgb565Pixel>())
    }

    fn buffer_mut(&mut self) -> &mut [Rgb565Pixel] {
        unsafe {
            std::slice::from_raw_parts_mut(
                self.mem.cast::<Rgb565Pixel>(),
                self.stride_pixels * self.height,
            )
        }
    }
}

impl Drop for HiddenRgb565Framebuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.mem.cast::<libc::c_void>(), self.map_len);
        }
    }
}

fn validate_hidden_map(
    index: HiddenRgb565BufferIndex,
    width: usize,
    height: usize,
    stride_bytes: usize,
) -> Result<HiddenMapSpec, HiddenFramebufferError> {
    if width == 0
        || height == 0
        || width.checked_mul(height).unwrap_or(usize::MAX) > MISTER_FB_SIZE_PIXELS
    {
        return Err(HiddenFramebufferError::InvalidGeometry { width, height });
    }
    let min_stride_bytes = rgb565_stride_bytes(width);
    if stride_bytes < min_stride_bytes {
        return Err(HiddenFramebufferError::InvalidStride {
            stride_bytes,
            min_stride_bytes,
        });
    }
    let map_len = stride_bytes
        .checked_mul(height)
        .ok_or(HiddenFramebufferError::AddressOverflow)?;
    if map_len > MISTER_FB_SLOT_BYTES {
        return Err(HiddenFramebufferError::MapTooLarge {
            map_len,
            slot_bytes: MISTER_FB_SLOT_BYTES,
        });
    }
    Ok(HiddenMapSpec {
        phys_addr: index.physical_address()?,
        map_len,
        stride_pixels: stride_bytes / std::mem::size_of::<Rgb565Pixel>(),
    })
}

fn hidden_buffer_physical_address(index: u8) -> Result<usize, HiddenFramebufferError> {
    HiddenRgb565BufferIndex::new(index)?;
    MISTER_FB_PHYS_BASE
        .checked_add(
            MISTER_FB_SLOT_BYTES
                .checked_mul(index as usize)
                .ok_or(HiddenFramebufferError::AddressOverflow)?,
        )
        .ok_or(HiddenFramebufferError::AddressOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_buffer_index_accepts_only_back_buffers() {
        assert!(HiddenRgb565BufferIndex::new(0).is_err());
        assert_eq!(HiddenRgb565BufferIndex::new(1).unwrap().get(), 1);
        assert_eq!(HiddenRgb565BufferIndex::new(2).unwrap().get(), 2);
        assert!(HiddenRgb565BufferIndex::new(3).is_err());
    }

    #[test]
    fn hidden_buffer_addresses_match_main_mister_slots() {
        assert_eq!(
            HiddenRgb565BufferIndex::new(1)
                .unwrap()
                .physical_address()
                .unwrap(),
            MISTER_FB_PHYS_BASE + MISTER_FB_SLOT_BYTES
        );
        assert_eq!(
            HiddenRgb565BufferIndex::new(2)
                .unwrap()
                .physical_address()
                .unwrap(),
            MISTER_FB_PHYS_BASE + MISTER_FB_SLOT_BYTES * 2
        );
    }

    #[test]
    fn hidden_map_validation_accepts_launcher_geometry() {
        let index = HiddenRgb565BufferIndex::new(1).unwrap();
        let spec = validate_hidden_map(index, 960, 540, rgb565_stride_bytes(960)).unwrap();

        assert_eq!(spec.map_len, rgb565_stride_bytes(960) * 540);
        assert_eq!(spec.stride_pixels, rgb565_stride_bytes(960) / 2);
    }

    #[test]
    fn hidden_map_validation_rejects_bad_geometry_and_stride() {
        let index = HiddenRgb565BufferIndex::new(1).unwrap();

        assert!(matches!(
            validate_hidden_map(index, 0, 540, rgb565_stride_bytes(960)),
            Err(HiddenFramebufferError::InvalidGeometry { .. })
        ));
        assert!(matches!(
            validate_hidden_map(index, 960, 540, 16),
            Err(HiddenFramebufferError::InvalidStride { .. })
        ));
    }
}
