//! Production mappings exposed by the stock-kernel scanout-slots module.
//!
//! The scanout-slot module maps framebuffer-owned physical ranges with write-combined
//! attributes. This module validates the reported regions before the launcher
//! uses them as Main-flippable hidden RGB565 buffers.

use crate::framebuffer::format::rgb565_stride_bytes;
use crate::framebuffer::hidden::{HiddenFramebufferError, HiddenRgb565BufferIndex};
use slint::platform::software_renderer::Rgb565Pixel;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;

pub const SCANOUT_SLOTS_DEVICE: &str = "/dev/mister-magik-scanout-slots";
pub const SCANOUT_SLOTS_MIN_VERSION: u32 = 1;
pub const SCANOUT_SLOTS_REGION_OFFSET_BYTES: usize = 1024 * 1024;
pub const SCANOUT_SLOT_FRAME_BYTES: usize = 960 * 540 * 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanoutSlotsHeader {
    pub name: String,
    pub version: u32,
    pub uts_release: String,
    pub region_offset_bytes: usize,
    pub cache_mode: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanoutSlotsRegion {
    pub index: usize,
    pub name: String,
    pub available: bool,
    pub phys: String,
    pub len: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanoutSlotsMetadata {
    pub header: ScanoutSlotsHeader,
    pub regions: Vec<ScanoutSlotsRegion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScanoutSlotsError {
    Io(String),
    MissingHeader,
    UnsupportedVersion {
        version: u32,
        min_version: u32,
    },
    UnsupportedRegionStride {
        region_offset_bytes: usize,
    },
    UnsupportedCacheMode {
        cache_mode: String,
    },
    MissingRegion {
        name: String,
    },
    RegionUnavailable {
        name: String,
    },
    RegionTooSmall {
        name: String,
        len: usize,
        required: usize,
    },
    InvalidGeometry(String),
    SourceTooShort {
        needed: usize,
        actual: usize,
    },
    MmapFailed(String),
    MmapReturnedNull,
    InvalidPhysicalAddress {
        name: String,
        phys: String,
    },
}

impl std::fmt::Display for ScanoutSlotsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "scanout slots I/O failed: {e}"),
            Self::MissingHeader => write!(f, "scanout slots metadata is missing a header"),
            Self::UnsupportedVersion {
                version,
                min_version,
            } => write!(
                f,
                "scanout slots version {version} is older than required version {min_version}"
            ),
            Self::UnsupportedRegionStride {
                region_offset_bytes,
            } => write!(
                f,
                "scanout slots region stride {region_offset_bytes} does not match expected {SCANOUT_SLOTS_REGION_OFFSET_BYTES}"
            ),
            Self::UnsupportedCacheMode { cache_mode } => {
                write!(f, "scanout slots cache mode {cache_mode} is not writecombine")
            }
            Self::MissingRegion { name } => write!(f, "scanout slots region {name} is missing"),
            Self::RegionUnavailable { name } => {
                write!(f, "scanout slots region {name} is unavailable")
            }
            Self::RegionTooSmall {
                name,
                len,
                required,
            } => write!(
                f,
                "scanout slots region {name} has {len} bytes, need {required}"
            ),
            Self::InvalidGeometry(e) => write!(f, "invalid scanout-slot framebuffer geometry: {e}"),
            Self::SourceTooShort { needed, actual } => {
                write!(f, "scanout-slot source has {actual} pixels, need {needed}")
            }
            Self::MmapFailed(e) => write!(f, "scanout slots mmap failed: {e}"),
            Self::MmapReturnedNull => write!(f, "scanout slots mmap returned a null address"),
            Self::InvalidPhysicalAddress { name, phys } => {
                write!(f, "scanout slots region {name} has invalid physical address {phys}")
            }
        }
    }
}

impl std::error::Error for ScanoutSlotsError {}

impl From<io::Error> for ScanoutSlotsError {
    fn from(value: io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

pub fn read_scanout_slots_metadata() -> Result<ScanoutSlotsMetadata, ScanoutSlotsError> {
    parse_scanout_slots_metadata(&fs::read_to_string(SCANOUT_SLOTS_DEVICE)?)
}

pub fn parse_scanout_slots_metadata(text: &str) -> Result<ScanoutSlotsMetadata, ScanoutSlotsError> {
    let header = text
        .lines()
        .find_map(parse_scanout_slots_header)
        .ok_or(ScanoutSlotsError::MissingHeader)?;
    let regions = text
        .lines()
        .filter_map(parse_scanout_slots_region)
        .collect();
    Ok(ScanoutSlotsMetadata { header, regions })
}

pub fn parse_scanout_slots_header(line: &str) -> Option<ScanoutSlotsHeader> {
    if !line.starts_with("scanout_slots_header_tsv\t") {
        return None;
    }
    let mut name = None;
    let mut version = None;
    let mut uts_release = None;
    let mut region_offset_bytes = None;
    let mut cache_mode = None;
    for field in line.split('\t').skip(1) {
        let (key, value) = field.split_once('=')?;
        match key {
            "name" => name = Some(value.to_string()),
            "version" => version = value.parse::<u32>().ok(),
            "uts_release" => uts_release = Some(value.to_string()),
            "region_offset_bytes" => region_offset_bytes = value.parse::<usize>().ok(),
            "cache_mode" => cache_mode = Some(value.to_string()),
            _ => {}
        }
    }
    Some(ScanoutSlotsHeader {
        name: name?,
        version: version?,
        uts_release: uts_release?,
        region_offset_bytes: region_offset_bytes?,
        cache_mode: cache_mode?,
    })
}

pub fn parse_scanout_slots_region(line: &str) -> Option<ScanoutSlotsRegion> {
    if !line.starts_with("scanout_slots_region_tsv\t") {
        return None;
    }
    let mut index = None;
    let mut name = None;
    let mut available = None;
    let mut phys = None;
    let mut len = None;
    for field in line.split('\t').skip(1) {
        let (key, value) = field.split_once('=')?;
        match key {
            "index" => index = value.parse::<usize>().ok(),
            "name" => name = Some(value.to_string()),
            "available" => available = Some(value == "1"),
            "phys" => phys = Some(value.to_string()),
            "len" => len = value.parse::<usize>().ok(),
            _ => {}
        }
    }
    Some(ScanoutSlotsRegion {
        index: index?,
        name: name?,
        available: available?,
        phys: phys?,
        len: len?,
    })
}

pub struct ScanoutSlotsRgb565Framebuffer {
    mem: *mut u8,
    map_len: usize,
    width: usize,
    height: usize,
    stride_pixels: usize,
    region: ScanoutSlotsRegion,
    _device: File,
}

impl ScanoutSlotsRgb565Framebuffer {
    pub fn open(
        index: HiddenRgb565BufferIndex,
        width: usize,
        height: usize,
        stride_bytes: usize,
    ) -> Result<Self, ScanoutSlotsError> {
        let metadata = read_scanout_slots_metadata()?;
        Self::open_with_metadata(index, width, height, stride_bytes, &metadata)
    }

    fn open_with_metadata(
        index: HiddenRgb565BufferIndex,
        width: usize,
        height: usize,
        stride_bytes: usize,
        metadata: &ScanoutSlotsMetadata,
    ) -> Result<Self, ScanoutSlotsError> {
        validate_scanout_slots_metadata(metadata)?;
        let map_len = validate_scanout_slots_geometry(width, height, stride_bytes)
            .map_err(|e| ScanoutSlotsError::InvalidGeometry(e.to_string()))?;
        let region = scanout_hidden_region(metadata, index, map_len)?;
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(SCANOUT_SLOTS_DEVICE)?;
        let offset = region
            .index
            .checked_mul(SCANOUT_SLOTS_REGION_OFFSET_BYTES)
            .ok_or(ScanoutSlotsError::UnsupportedRegionStride {
                region_offset_bytes: usize::MAX,
            })?;
        let mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                device.as_raw_fd(),
                offset as libc::off_t,
            )
        };
        if mem == libc::MAP_FAILED {
            return Err(ScanoutSlotsError::MmapFailed(
                io::Error::last_os_error().to_string(),
            ));
        }
        if mem.is_null() {
            unsafe {
                libc::munmap(mem, map_len);
            }
            return Err(ScanoutSlotsError::MmapReturnedNull);
        }
        Ok(Self {
            mem: mem.cast::<u8>(),
            map_len,
            width,
            height,
            stride_pixels: stride_bytes / std::mem::size_of::<Rgb565Pixel>(),
            region,
            _device: device,
        })
    }

    pub fn copy_full_frame(
        &mut self,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
    ) -> Result<usize, ScanoutSlotsError> {
        if src_stride_pixels < self.width {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than width {}",
                self.width
            )));
        }
        let needed = src_stride_pixels.checked_mul(self.height).ok_or(
            ScanoutSlotsError::InvalidGeometry("source size overflow".to_string()),
        )?;
        if src.len() < needed {
            return Err(ScanoutSlotsError::SourceTooShort {
                needed,
                actual: src.len(),
            });
        }
        let width = self.width;
        let height = self.height;
        let stride_pixels = self.stride_pixels;
        let dst = self.buffer_mut();
        copy_full_frame_pixels(dst, stride_pixels, src, src_stride_pixels, width, height);
        Ok(stride_pixels * height * std::mem::size_of::<Rgb565Pixel>())
    }

    pub fn copy_rect(
        &mut self,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
        rect: crate::framebuffer::target::DirtyRect,
    ) -> Result<usize, ScanoutSlotsError> {
        if rect.x1 > self.width || rect.y1 > self.height {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "rect x0={} y0={} x1={} y1={} exceeds {}x{}",
                rect.x0, rect.y0, rect.x1, rect.y1, self.width, self.height
            )));
        }
        if src_stride_pixels < self.width {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than width {}",
                self.width
            )));
        }
        let needed = src_stride_pixels.checked_mul(self.height).ok_or(
            ScanoutSlotsError::InvalidGeometry("source size overflow".to_string()),
        )?;
        if src.len() < needed {
            return Err(ScanoutSlotsError::SourceTooShort {
                needed,
                actual: src.len(),
            });
        }
        if rect.x0 >= rect.x1 || rect.y0 >= rect.y1 {
            return Ok(0);
        }
        let stride_pixels = self.stride_pixels;
        let dst = self.buffer_mut();
        copy_rect_pixels(dst, stride_pixels, src, src_stride_pixels, rect);
        Ok(rect.width() * (rect.y1 - rect.y0) * std::mem::size_of::<Rgb565Pixel>())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn copy_rect_565_strided(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
        src_x: usize,
        src_y: usize,
    ) -> Result<usize, ScanoutSlotsError> {
        if w == 0 || h == 0 {
            return Ok(0);
        }
        let x1 = x
            .checked_add(w)
            .ok_or_else(|| ScanoutSlotsError::InvalidGeometry("target x overflow".to_string()))?;
        let y1 = y
            .checked_add(h)
            .ok_or_else(|| ScanoutSlotsError::InvalidGeometry("target y overflow".to_string()))?;
        if x1 > self.width || y1 > self.height {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "target x={x} y={y} w={w} h={h} exceeds {}x{}",
                self.width, self.height
            )));
        }
        let src_x1 = src_x
            .checked_add(w)
            .ok_or_else(|| ScanoutSlotsError::InvalidGeometry("source x overflow".to_string()))?;
        let src_y1 = src_y
            .checked_add(h)
            .ok_or_else(|| ScanoutSlotsError::InvalidGeometry("source y overflow".to_string()))?;
        if src_stride_pixels < src_x1 {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than source x+w {src_x1}"
            )));
        }
        let needed =
            src_stride_pixels
                .checked_mul(src_y1)
                .ok_or(ScanoutSlotsError::InvalidGeometry(
                    "source size overflow".to_string(),
                ))?;
        if src.len() < needed {
            return Err(ScanoutSlotsError::SourceTooShort {
                needed,
                actual: src.len(),
            });
        }
        let dst_stride_pixels = self.stride_pixels;
        let dst = self.buffer_mut();
        copy_rect_565_strided_pixels(
            dst,
            dst_stride_pixels,
            x,
            y,
            w,
            h,
            src,
            src_stride_pixels,
            src_x,
            src_y,
        );
        Ok(w * h * std::mem::size_of::<Rgb565Pixel>())
    }

    pub fn region(&self) -> &ScanoutSlotsRegion {
        &self.region
    }

    pub fn physical_addr(&self) -> Result<u32, ScanoutSlotsError> {
        parse_region_phys_u32(&self.region)
    }

    pub fn pixels(&self) -> &[Rgb565Pixel] {
        unsafe {
            std::slice::from_raw_parts(
                self.mem.cast::<Rgb565Pixel>(),
                self.stride_pixels * self.height,
            )
        }
    }

    pub fn stride_pixels(&self) -> usize {
        self.stride_pixels
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

fn copy_full_frame_pixels(
    dst: &mut [Rgb565Pixel],
    dst_stride_pixels: usize,
    src: &[Rgb565Pixel],
    src_stride_pixels: usize,
    width: usize,
    height: usize,
) {
    if src_stride_pixels == width && dst_stride_pixels == width {
        let len = width * height;
        dst[..len].copy_from_slice(&src[..len]);
        return;
    }
    for y in 0..height {
        let src_start = y * src_stride_pixels;
        let dst_start = y * dst_stride_pixels;
        dst[dst_start..dst_start + width].copy_from_slice(&src[src_start..src_start + width]);
    }
}

fn copy_rect_pixels(
    dst: &mut [Rgb565Pixel],
    dst_stride_pixels: usize,
    src: &[Rgb565Pixel],
    src_stride_pixels: usize,
    rect: crate::framebuffer::target::DirtyRect,
) {
    for y in rect.y0..rect.y1 {
        let src_start = y * src_stride_pixels + rect.x0;
        let dst_start = y * dst_stride_pixels + rect.x0;
        dst[dst_start..dst_start + rect.width()]
            .copy_from_slice(&src[src_start..src_start + rect.width()]);
    }
}

#[allow(clippy::too_many_arguments)]
fn copy_rect_565_strided_pixels(
    dst: &mut [Rgb565Pixel],
    dst_stride_pixels: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    src: &[Rgb565Pixel],
    src_stride_pixels: usize,
    src_x: usize,
    src_y: usize,
) {
    for row in 0..h {
        let src_start = (src_y + row) * src_stride_pixels + src_x;
        let dst_start = (y + row) * dst_stride_pixels + x;
        dst[dst_start..dst_start + w].copy_from_slice(&src[src_start..src_start + w]);
    }
}

pub fn parse_region_phys_u32(region: &ScanoutSlotsRegion) -> Result<u32, ScanoutSlotsError> {
    let raw = region.phys.trim();
    let hex = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(raw);
    u32::from_str_radix(hex, 16).map_err(|_| ScanoutSlotsError::InvalidPhysicalAddress {
        name: region.name.clone(),
        phys: region.phys.clone(),
    })
}

impl Drop for ScanoutSlotsRgb565Framebuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.mem.cast::<libc::c_void>(), self.map_len);
        }
    }
}

fn validate_scanout_slots_metadata(
    metadata: &ScanoutSlotsMetadata,
) -> Result<(), ScanoutSlotsError> {
    if metadata.header.version < SCANOUT_SLOTS_MIN_VERSION {
        return Err(ScanoutSlotsError::UnsupportedVersion {
            version: metadata.header.version,
            min_version: SCANOUT_SLOTS_MIN_VERSION,
        });
    }
    if metadata.header.region_offset_bytes != SCANOUT_SLOTS_REGION_OFFSET_BYTES {
        return Err(ScanoutSlotsError::UnsupportedRegionStride {
            region_offset_bytes: metadata.header.region_offset_bytes,
        });
    }
    if metadata.header.cache_mode != "writecombine" {
        return Err(ScanoutSlotsError::UnsupportedCacheMode {
            cache_mode: metadata.header.cache_mode.clone(),
        });
    }
    Ok(())
}

fn validate_scanout_slots_geometry(
    width: usize,
    height: usize,
    stride_bytes: usize,
) -> Result<usize, HiddenFramebufferError> {
    if width == 0 || height == 0 {
        return Err(HiddenFramebufferError::InvalidGeometry { width, height });
    }
    let min_stride_bytes = rgb565_stride_bytes(width);
    if stride_bytes < min_stride_bytes {
        return Err(HiddenFramebufferError::InvalidStride {
            stride_bytes,
            min_stride_bytes,
        });
    }
    stride_bytes
        .checked_mul(height)
        .ok_or(HiddenFramebufferError::AddressOverflow)
}

fn scanout_hidden_region(
    metadata: &ScanoutSlotsMetadata,
    index: HiddenRgb565BufferIndex,
    required_len: usize,
) -> Result<ScanoutSlotsRegion, ScanoutSlotsError> {
    let name = format!("hidden-slot-{}", index.get());
    let region = metadata
        .regions
        .iter()
        .find(|region| region.name == name)
        .cloned()
        .ok_or_else(|| ScanoutSlotsError::MissingRegion { name: name.clone() })?;
    if !region.available {
        return Err(ScanoutSlotsError::RegionUnavailable { name });
    }
    if region.len < required_len {
        return Err(ScanoutSlotsError::RegionTooSmall {
            name,
            len: region.len,
            required: required_len,
        });
    }
    Ok(region)
}

#[cfg(test)]
mod tests {
    use crate::framebuffer::target::DirtyRect;

    use super::*;

    fn metadata() -> ScanoutSlotsMetadata {
        parse_scanout_slots_metadata(
            "\
scanout_slots_header_tsv\tname=mister-magik-scanout-slots\tversion=1\tuts_release=5.15.1-MiSTer\topen_count=1\tmmap_count=0\tpage_size=4096\tregion_offset_pages=256\tregion_offset_bytes=1048576\tcache_mode=writecombine\n\
scanout_slots_region_tsv\tindex=0\tname=hidden-slot-1\tavailable=1\tphys=0x22800000\tlen=1036800\n\
scanout_slots_region_tsv\tindex=1\tname=hidden-slot-2\tavailable=1\tphys=0x23000000\tlen=1036800\n",
        )
        .unwrap()
    }

    #[test]
    fn parser_reads_header_and_regions() {
        let metadata = metadata();

        assert_eq!(metadata.header.version, 1);
        assert_eq!(
            metadata.header.region_offset_bytes,
            SCANOUT_SLOTS_REGION_OFFSET_BYTES
        );
        assert_eq!(metadata.header.cache_mode, "writecombine");
        assert_eq!(metadata.regions[0].name, "hidden-slot-1");
    }

    #[test]
    fn validation_rejects_old_or_non_wc_contracts() {
        let mut old_metadata = metadata();
        old_metadata.header.version = 0;
        assert!(matches!(
            validate_scanout_slots_metadata(&old_metadata),
            Err(ScanoutSlotsError::UnsupportedVersion { .. })
        ));

        let mut uncached_metadata = metadata();
        uncached_metadata.header.cache_mode = "uncached".to_string();
        assert!(matches!(
            validate_scanout_slots_metadata(&uncached_metadata),
            Err(ScanoutSlotsError::UnsupportedCacheMode { .. })
        ));
    }

    #[test]
    fn hidden_region_selects_available_slots() {
        let metadata = metadata();
        let region = scanout_hidden_region(
            &metadata,
            HiddenRgb565BufferIndex::new(2).unwrap(),
            SCANOUT_SLOT_FRAME_BYTES,
        )
        .unwrap();

        assert_eq!(region.index, 1);
        assert_eq!(region.name, "hidden-slot-2");
    }

    #[test]
    fn hidden_region_rejects_short_or_missing_slots() {
        let mut metadata = metadata();
        metadata.regions[0].len = 16;
        assert!(matches!(
            scanout_hidden_region(
                &metadata,
                HiddenRgb565BufferIndex::new(1).unwrap(),
                SCANOUT_SLOT_FRAME_BYTES
            ),
            Err(ScanoutSlotsError::RegionTooSmall { .. })
        ));

        metadata
            .regions
            .retain(|region| region.name != "hidden-slot-1");
        assert!(matches!(
            scanout_hidden_region(
                &metadata,
                HiddenRgb565BufferIndex::new(1).unwrap(),
                SCANOUT_SLOT_FRAME_BYTES
            ),
            Err(ScanoutSlotsError::MissingRegion { .. })
        ));
    }

    #[test]
    fn full_frame_copy_uses_contiguous_geometry() {
        let src: Vec<Rgb565Pixel> = (0..12).map(Rgb565Pixel).collect();
        let mut dst = vec![Rgb565Pixel(0); 12];

        copy_full_frame_pixels(&mut dst, 4, &src, 4, 4, 3);

        assert_eq!(dst, src);
    }

    #[test]
    fn full_frame_copy_preserves_padded_destination_rows() {
        let src = vec![
            Rgb565Pixel(1),
            Rgb565Pixel(2),
            Rgb565Pixel(99),
            Rgb565Pixel(3),
            Rgb565Pixel(4),
            Rgb565Pixel(98),
        ];
        let mut dst = vec![Rgb565Pixel(0); 8];

        copy_full_frame_pixels(&mut dst, 4, &src, 3, 2, 2);

        assert_eq!(
            dst,
            vec![
                Rgb565Pixel(1),
                Rgb565Pixel(2),
                Rgb565Pixel(0),
                Rgb565Pixel(0),
                Rgb565Pixel(3),
                Rgb565Pixel(4),
                Rgb565Pixel(0),
                Rgb565Pixel(0),
            ]
        );
    }

    #[test]
    fn rect_copy_updates_only_requested_region() {
        let src: Vec<Rgb565Pixel> = (0..16).map(Rgb565Pixel).collect();
        let mut dst = vec![Rgb565Pixel(99); 16];

        copy_rect_pixels(
            &mut dst,
            4,
            &src,
            4,
            DirtyRect {
                x0: 1,
                y0: 1,
                x1: 3,
                y1: 3,
            },
        );

        assert_eq!(
            dst,
            vec![
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(5),
                Rgb565Pixel(6),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(9),
                Rgb565Pixel(10),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
            ]
        );
    }

    #[test]
    fn strided_rect_copy_updates_destination_offset() {
        let src: Vec<Rgb565Pixel> = (0..30).map(Rgb565Pixel).collect();
        let mut dst = vec![Rgb565Pixel(99); 24];

        copy_rect_565_strided_pixels(&mut dst, 6, 2, 1, 3, 2, &src, 5, 1, 3);

        assert_eq!(
            dst,
            vec![
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(16),
                Rgb565Pixel(17),
                Rgb565Pixel(18),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(21),
                Rgb565Pixel(22),
                Rgb565Pixel(23),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
            ]
        );
    }
}
