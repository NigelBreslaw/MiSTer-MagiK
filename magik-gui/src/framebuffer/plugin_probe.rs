//! Experimental mappings exposed by the stock-kernel plugin probe.
//!
//! The plugin maps framebuffer-owned physical ranges with write-combined
//! attributes. This module validates the reported regions before the launcher
//! uses them as Main-flippable hidden RGB565 buffers.

use crate::framebuffer::format::rgb565_stride_bytes;
use crate::framebuffer::hidden::{HiddenFramebufferError, HiddenRgb565BufferIndex};
use slint::platform::software_renderer::Rgb565Pixel;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;

pub const PLUGIN_PROBE_DEVICE: &str = "/dev/mister-magik-plugin-probe";
pub const PLUGIN_PROBE_MIN_VERSION: u32 = 2;
pub const PLUGIN_PROBE_REGION_OFFSET_BYTES: usize = 1024 * 1024;
pub const PLUGIN_HIDDEN_SLOT_FRAME_BYTES: usize = 960 * 540 * 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginProbeHeader {
    pub name: String,
    pub version: u32,
    pub uts_release: String,
    pub region_offset_bytes: usize,
    pub cache_mode: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginProbeRegion {
    pub index: usize,
    pub name: String,
    pub available: bool,
    pub phys: String,
    pub len: usize,
    pub dma_owned: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginProbeMetadata {
    pub header: PluginProbeHeader,
    pub regions: Vec<PluginProbeRegion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginProbeError {
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
    RegionIsDmaOwned {
        name: String,
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

impl std::fmt::Display for PluginProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "plugin probe I/O failed: {e}"),
            Self::MissingHeader => write!(f, "plugin probe metadata is missing a header"),
            Self::UnsupportedVersion {
                version,
                min_version,
            } => write!(
                f,
                "plugin probe version {version} is older than required version {min_version}"
            ),
            Self::UnsupportedRegionStride {
                region_offset_bytes,
            } => write!(
                f,
                "plugin probe region stride {region_offset_bytes} does not match expected {PLUGIN_PROBE_REGION_OFFSET_BYTES}"
            ),
            Self::UnsupportedCacheMode { cache_mode } => {
                write!(f, "plugin probe cache mode {cache_mode} is not writecombine")
            }
            Self::MissingRegion { name } => write!(f, "plugin probe region {name} is missing"),
            Self::RegionUnavailable { name } => {
                write!(f, "plugin probe region {name} is unavailable")
            }
            Self::RegionTooSmall {
                name,
                len,
                required,
            } => write!(
                f,
                "plugin probe region {name} has {len} bytes, need {required}"
            ),
            Self::RegionIsDmaOwned { name } => {
                write!(f, "plugin probe region {name} is plugin-owned DMA memory")
            }
            Self::InvalidGeometry(e) => write!(f, "invalid plugin hidden framebuffer geometry: {e}"),
            Self::SourceTooShort { needed, actual } => {
                write!(f, "plugin hidden source has {actual} pixels, need {needed}")
            }
            Self::MmapFailed(e) => write!(f, "plugin probe mmap failed: {e}"),
            Self::MmapReturnedNull => write!(f, "plugin probe mmap returned a null address"),
            Self::InvalidPhysicalAddress { name, phys } => {
                write!(f, "plugin probe region {name} has invalid physical address {phys}")
            }
        }
    }
}

impl std::error::Error for PluginProbeError {}

impl From<io::Error> for PluginProbeError {
    fn from(value: io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

pub fn read_plugin_probe_metadata() -> Result<PluginProbeMetadata, PluginProbeError> {
    parse_plugin_probe_metadata(&fs::read_to_string(PLUGIN_PROBE_DEVICE)?)
}

pub fn parse_plugin_probe_metadata(text: &str) -> Result<PluginProbeMetadata, PluginProbeError> {
    let header = text
        .lines()
        .find_map(parse_plugin_probe_header)
        .ok_or(PluginProbeError::MissingHeader)?;
    let regions = text.lines().filter_map(parse_plugin_probe_region).collect();
    Ok(PluginProbeMetadata { header, regions })
}

pub fn parse_plugin_probe_header(line: &str) -> Option<PluginProbeHeader> {
    if !line.starts_with("plugin_probe_header_tsv\t") {
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
    Some(PluginProbeHeader {
        name: name?,
        version: version?,
        uts_release: uts_release?,
        region_offset_bytes: region_offset_bytes?,
        cache_mode: cache_mode?,
    })
}

pub fn parse_plugin_probe_region(line: &str) -> Option<PluginProbeRegion> {
    if !line.starts_with("plugin_probe_region_tsv\t") {
        return None;
    }
    let mut index = None;
    let mut name = None;
    let mut available = None;
    let mut phys = None;
    let mut len = None;
    let mut dma_owned = None;
    for field in line.split('\t').skip(1) {
        let (key, value) = field.split_once('=')?;
        match key {
            "index" => index = value.parse::<usize>().ok(),
            "name" => name = Some(value.to_string()),
            "available" => available = Some(value == "1"),
            "phys" => phys = Some(value.to_string()),
            "len" => len = value.parse::<usize>().ok(),
            "dma_owned" => dma_owned = Some(value == "1"),
            _ => {}
        }
    }
    Some(PluginProbeRegion {
        index: index?,
        name: name?,
        available: available?,
        phys: phys?,
        len: len?,
        dma_owned: dma_owned?,
    })
}

pub struct PluginHiddenRgb565Framebuffer {
    mem: *mut u8,
    map_len: usize,
    width: usize,
    height: usize,
    stride_pixels: usize,
    region: PluginProbeRegion,
    _device: File,
}

impl PluginHiddenRgb565Framebuffer {
    pub fn open(
        index: HiddenRgb565BufferIndex,
        width: usize,
        height: usize,
        stride_bytes: usize,
    ) -> Result<Self, PluginProbeError> {
        let metadata = read_plugin_probe_metadata()?;
        Self::open_with_metadata(index, width, height, stride_bytes, &metadata)
    }

    fn open_with_metadata(
        index: HiddenRgb565BufferIndex,
        width: usize,
        height: usize,
        stride_bytes: usize,
        metadata: &PluginProbeMetadata,
    ) -> Result<Self, PluginProbeError> {
        validate_plugin_metadata(metadata)?;
        let map_len = validate_plugin_geometry(width, height, stride_bytes)
            .map_err(|e| PluginProbeError::InvalidGeometry(e.to_string()))?;
        let region = plugin_hidden_region(metadata, index, map_len)?;
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(PLUGIN_PROBE_DEVICE)?;
        let offset = region
            .index
            .checked_mul(PLUGIN_PROBE_REGION_OFFSET_BYTES)
            .ok_or(PluginProbeError::UnsupportedRegionStride {
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
            return Err(PluginProbeError::MmapFailed(
                io::Error::last_os_error().to_string(),
            ));
        }
        if mem.is_null() {
            unsafe {
                libc::munmap(mem, map_len);
            }
            return Err(PluginProbeError::MmapReturnedNull);
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
    ) -> Result<usize, PluginProbeError> {
        if src_stride_pixels < self.width {
            return Err(PluginProbeError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than width {}",
                self.width
            )));
        }
        let needed =
            src_stride_pixels
                .checked_mul(self.height)
                .ok_or(PluginProbeError::InvalidGeometry(
                    "source size overflow".to_string(),
                ))?;
        if src.len() < needed {
            return Err(PluginProbeError::SourceTooShort {
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
    ) -> Result<usize, PluginProbeError> {
        if rect.x1 > self.width || rect.y1 > self.height {
            return Err(PluginProbeError::InvalidGeometry(format!(
                "rect x0={} y0={} x1={} y1={} exceeds {}x{}",
                rect.x0, rect.y0, rect.x1, rect.y1, self.width, self.height
            )));
        }
        if src_stride_pixels < self.width {
            return Err(PluginProbeError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than width {}",
                self.width
            )));
        }
        let needed =
            src_stride_pixels
                .checked_mul(self.height)
                .ok_or(PluginProbeError::InvalidGeometry(
                    "source size overflow".to_string(),
                ))?;
        if src.len() < needed {
            return Err(PluginProbeError::SourceTooShort {
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
    ) -> Result<usize, PluginProbeError> {
        if w == 0 || h == 0 {
            return Ok(0);
        }
        let x1 = x
            .checked_add(w)
            .ok_or_else(|| PluginProbeError::InvalidGeometry("target x overflow".to_string()))?;
        let y1 = y
            .checked_add(h)
            .ok_or_else(|| PluginProbeError::InvalidGeometry("target y overflow".to_string()))?;
        if x1 > self.width || y1 > self.height {
            return Err(PluginProbeError::InvalidGeometry(format!(
                "target x={x} y={y} w={w} h={h} exceeds {}x{}",
                self.width, self.height
            )));
        }
        let src_x1 = src_x
            .checked_add(w)
            .ok_or_else(|| PluginProbeError::InvalidGeometry("source x overflow".to_string()))?;
        let src_y1 = src_y
            .checked_add(h)
            .ok_or_else(|| PluginProbeError::InvalidGeometry("source y overflow".to_string()))?;
        if src_stride_pixels < src_x1 {
            return Err(PluginProbeError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than source x+w {src_x1}"
            )));
        }
        let needed =
            src_stride_pixels
                .checked_mul(src_y1)
                .ok_or(PluginProbeError::InvalidGeometry(
                    "source size overflow".to_string(),
                ))?;
        if src.len() < needed {
            return Err(PluginProbeError::SourceTooShort {
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

    pub fn region(&self) -> &PluginProbeRegion {
        &self.region
    }

    pub fn physical_addr(&self) -> Result<u32, PluginProbeError> {
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

pub fn parse_region_phys_u32(region: &PluginProbeRegion) -> Result<u32, PluginProbeError> {
    let raw = region.phys.trim();
    let hex = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(raw);
    u32::from_str_radix(hex, 16).map_err(|_| PluginProbeError::InvalidPhysicalAddress {
        name: region.name.clone(),
        phys: region.phys.clone(),
    })
}

impl Drop for PluginHiddenRgb565Framebuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.mem.cast::<libc::c_void>(), self.map_len);
        }
    }
}

fn validate_plugin_metadata(metadata: &PluginProbeMetadata) -> Result<(), PluginProbeError> {
    if metadata.header.version < PLUGIN_PROBE_MIN_VERSION {
        return Err(PluginProbeError::UnsupportedVersion {
            version: metadata.header.version,
            min_version: PLUGIN_PROBE_MIN_VERSION,
        });
    }
    if metadata.header.region_offset_bytes != PLUGIN_PROBE_REGION_OFFSET_BYTES {
        return Err(PluginProbeError::UnsupportedRegionStride {
            region_offset_bytes: metadata.header.region_offset_bytes,
        });
    }
    if metadata.header.cache_mode != "writecombine" {
        return Err(PluginProbeError::UnsupportedCacheMode {
            cache_mode: metadata.header.cache_mode.clone(),
        });
    }
    Ok(())
}

fn validate_plugin_geometry(
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

fn plugin_hidden_region(
    metadata: &PluginProbeMetadata,
    index: HiddenRgb565BufferIndex,
    required_len: usize,
) -> Result<PluginProbeRegion, PluginProbeError> {
    let name = format!("hidden-slot-{}", index.get());
    let region = metadata
        .regions
        .iter()
        .find(|region| region.name == name)
        .cloned()
        .ok_or_else(|| PluginProbeError::MissingRegion { name: name.clone() })?;
    if !region.available {
        return Err(PluginProbeError::RegionUnavailable { name });
    }
    if region.dma_owned {
        return Err(PluginProbeError::RegionIsDmaOwned { name });
    }
    if region.len < required_len {
        return Err(PluginProbeError::RegionTooSmall {
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

    fn metadata() -> PluginProbeMetadata {
        parse_plugin_probe_metadata(
            "\
plugin_probe_header_tsv\tname=mister-magik-plugin-probe\tversion=2\tuts_release=5.15.1-MiSTer\topen_count=1\tmmap_count=0\tpage_size=4096\tregion_offset_pages=256\tregion_offset_bytes=1048576\tcache_mode=writecombine\n\
plugin_probe_region_tsv\tindex=0\tname=adjacent-fb-resource\tavailable=1\tphys=0x220fd200\tlen=1036800\tdma_owned=0\n\
plugin_probe_region_tsv\tindex=1\tname=hidden-slot-1\tavailable=1\tphys=0x22800000\tlen=1036800\tdma_owned=0\n\
plugin_probe_region_tsv\tindex=2\tname=hidden-slot-2\tavailable=1\tphys=0x23000000\tlen=1036800\tdma_owned=0\n\
plugin_probe_region_tsv\tindex=3\tname=plugin-owned-dma\tavailable=0\tphys=0x00000000\tlen=1036800\tdma_owned=1\n",
        )
        .unwrap()
    }

    #[test]
    fn parser_reads_header_and_regions() {
        let metadata = metadata();

        assert_eq!(metadata.header.version, 2);
        assert_eq!(
            metadata.header.region_offset_bytes,
            PLUGIN_PROBE_REGION_OFFSET_BYTES
        );
        assert_eq!(metadata.header.cache_mode, "writecombine");
        assert_eq!(metadata.regions[1].name, "hidden-slot-1");
    }

    #[test]
    fn validation_rejects_old_or_non_wc_contracts() {
        let mut old_metadata = metadata();
        old_metadata.header.version = 1;
        assert!(matches!(
            validate_plugin_metadata(&old_metadata),
            Err(PluginProbeError::UnsupportedVersion { .. })
        ));

        let mut uncached_metadata = metadata();
        uncached_metadata.header.cache_mode = "uncached".to_string();
        assert!(matches!(
            validate_plugin_metadata(&uncached_metadata),
            Err(PluginProbeError::UnsupportedCacheMode { .. })
        ));
    }

    #[test]
    fn hidden_region_selects_only_available_non_dma_slots() {
        let metadata = metadata();
        let region = plugin_hidden_region(
            &metadata,
            HiddenRgb565BufferIndex::new(2).unwrap(),
            PLUGIN_HIDDEN_SLOT_FRAME_BYTES,
        )
        .unwrap();

        assert_eq!(region.index, 2);
        assert_eq!(region.name, "hidden-slot-2");
    }

    #[test]
    fn hidden_region_rejects_short_or_missing_slots() {
        let mut metadata = metadata();
        metadata.regions[1].len = 16;
        assert!(matches!(
            plugin_hidden_region(
                &metadata,
                HiddenRgb565BufferIndex::new(1).unwrap(),
                PLUGIN_HIDDEN_SLOT_FRAME_BYTES
            ),
            Err(PluginProbeError::RegionTooSmall { .. })
        ));

        metadata
            .regions
            .retain(|region| region.name != "hidden-slot-1");
        assert!(matches!(
            plugin_hidden_region(
                &metadata,
                HiddenRgb565BufferIndex::new(1).unwrap(),
                PLUGIN_HIDDEN_SLOT_FRAME_BYTES
            ),
            Err(PluginProbeError::MissingRegion { .. })
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
