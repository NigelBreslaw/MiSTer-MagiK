// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read-only exFAT directory shadow reader used by catalog diagnostics.
//!
//! The kernel namespace walker remains authoritative.  This module is only
//! enabled by an explicit diagnostic environment variable and is deliberately
//! conservative: any geometry, allocation, decoding, or parity uncertainty
//! returns an error so callers can keep using the normal walker.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
#[cfg(target_os = "linux")]
use std::os::unix::fs::FileTypeExt;
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Instant;

const EXFAT_OEM: &[u8; 8] = b"EXFAT   ";
const MAX_CHAIN_CLUSTERS: usize = 32_768;
const MAX_DIRECTORY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 4_000_000;
const MAX_DIRECTORY_DEPTH: usize = 256;
const FILE_ENTRY: u8 = 0x85;
const STREAM_ENTRY: u8 = 0xc0;
const NAME_ENTRY: u8 = 0xc1;
const LAST_CLUSTER: u32 = 0xffff_fff8;
pub(crate) const SHADOW_ENV: &str = "MISTER_CATALOG_EXFAT_SHADOW";

static SHADOW_CACHE: OnceLock<Result<Option<Arc<ShadowReport>>, String>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ShadowEntryKind {
    Directory,
    File,
}

#[derive(Clone, Debug)]
pub(crate) struct ShadowEntry {
    pub(crate) kind: ShadowEntryKind,
    pub(crate) size: u64,
    pub(crate) first_cluster: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct ShadowReport {
    pub(crate) mountpoint: PathBuf,
    pub(crate) device: PathBuf,
    pub(crate) entries: BTreeMap<PathBuf, ShadowEntry>,
    pub(crate) directories: usize,
    pub(crate) requests: usize,
    pub(crate) bytes_read: u64,
    pub(crate) elapsed_us: u64,
}

#[derive(Clone, Debug)]
struct Geometry {
    sector_size: u64,
    sectors_per_cluster: u64,
    fat_offset: u64,
    fat_length: u64,
    heap_offset: u64,
    cluster_count: u64,
    volume_length: u64,
    root_cluster: u32,
}

struct Reader {
    file: File,
    geometry: Geometry,
    requests: usize,
    bytes_read: u64,
    entries: BTreeMap<PathBuf, ShadowEntry>,
    directories: usize,
}

#[derive(Clone, Debug)]
struct FileSet {
    name: String,
    kind: ShadowEntryKind,
    size: u64,
    first_cluster: u32,
    no_fat_chain: bool,
}

/// Read the mounted exFAT volume containing `path` once and return its full
/// directory index. `Ok(None)` means the path is not on an exFAT block source
/// (or mount information is unavailable); callers must use the kernel walker.
pub(crate) fn shadow_path(path: &Path) -> Result<Option<Arc<ShadowReport>>, String> {
    let Some((mountpoint, device)) = mounted_block_source(path)? else {
        return Ok(None);
    };
    let started = Instant::now();
    let file = File::open(&device)
        .map_err(|error| format!("open exFAT backing device {}: {error}", device.display()))?;
    let geometry = match read_geometry(&file)? {
        Some(geometry) => geometry,
        None => return Ok(None),
    };
    let mut reader = Reader {
        file,
        geometry,
        requests: 0,
        bytes_read: 0,
        entries: BTreeMap::new(),
        directories: 0,
    };
    let relative = path
        .strip_prefix(&mountpoint)
        .map_err(|_| "exFAT shadow path is outside its mountpoint".to_string())?;
    let components = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    reader
        .walk_to_target(reader.geometry.root_cluster, &mountpoint, &components, 0)
        .map_err(|error| format!("{error} (device {})", device.display()))?;
    let report = ShadowReport {
        mountpoint,
        device,
        entries: reader.entries,
        directories: reader.directories,
        requests: reader.requests,
        bytes_read: reader.bytes_read,
        elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
    };
    Ok(Some(Arc::new(report)))
}

/// Return the one diagnostic shadow index requested by the environment. A
/// literal path limits the expensive raw read to one game directory; `1`
/// selects the first matching path encountered by the builder. The index is
/// cached so a full-volume read is never repeated for every generic system.
pub(crate) fn configured_shadow(path: &Path) -> Result<Option<Arc<ShadowReport>>, String> {
    let Some(value) = std::env::var_os(SHADOW_ENV) else {
        return Ok(None);
    };
    let configured = value.to_string_lossy();
    if configured != "1" && Path::new(configured.as_ref()) != path {
        return Ok(None);
    }
    SHADOW_CACHE.get_or_init(|| shadow_path(path)).clone()
}

fn read_geometry(file: &File) -> Result<Option<Geometry>, String> {
    let mut boot = [0_u8; 512];
    read_at(file, &mut boot, 0, "exFAT boot sector")?;
    if &boot[3..11] != EXFAT_OEM {
        return Ok(None);
    }
    let volume_length = le_u64(&boot, 72);
    let fat_offset = u64::from(le_u32(&boot, 80));
    let fat_length = u64::from(le_u32(&boot, 84));
    let heap_offset = u64::from(le_u32(&boot, 88));
    let cluster_count = u64::from(le_u32(&boot, 92));
    let root_cluster = le_u32(&boot, 96);
    let sector_shift = boot[108];
    let cluster_shift = boot[109];
    if !(9..=12).contains(&sector_shift) || cluster_shift > 25 {
        return Err(format!(
            "unsupported exFAT geometry: sector_shift={sector_shift} cluster_shift={cluster_shift}"
        ));
    }
    let sector_size = 1_u64 << sector_shift;
    let sectors_per_cluster = 1_u64
        .checked_shl(u32::from(cluster_shift))
        .ok_or("exFAT sectors-per-cluster shift overflow")?;
    if volume_length == 0
        || fat_length == 0
        || cluster_count == 0
        || root_cluster < 2
        || u64::from(root_cluster) > cluster_count + 1
    {
        return Err("invalid exFAT boot-sector bounds".to_string());
    }
    let fat_end = fat_offset
        .checked_add(fat_length)
        .ok_or("exFAT FAT range overflow")?;
    let heap_sectors = cluster_count
        .checked_mul(sectors_per_cluster)
        .ok_or("exFAT heap range overflow")?;
    let heap_end = heap_offset
        .checked_add(heap_sectors)
        .ok_or("exFAT heap range overflow")?;
    if fat_end > volume_length || heap_end > volume_length || heap_offset < fat_end {
        return Err("exFAT FAT/heap range exceeds the volume".to_string());
    }
    let cluster_bytes = sectors_per_cluster
        .checked_mul(sector_size)
        .ok_or("exFAT cluster size overflow")?;
    if cluster_bytes == 0 || cluster_bytes > MAX_DIRECTORY_BYTES {
        return Err(format!("exFAT cluster is too large: {cluster_bytes} bytes"));
    }
    Ok(Some(Geometry {
        sector_size,
        sectors_per_cluster,
        fat_offset,
        fat_length,
        heap_offset,
        cluster_count,
        volume_length,
        root_cluster,
    }))
}

impl Reader {
    fn cluster_bytes(&self) -> Result<usize, String> {
        self.geometry
            .sector_size
            .checked_mul(self.geometry.sectors_per_cluster)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or("exFAT cluster size does not fit usize".to_string())
    }

    fn cluster_offset(&self, cluster: u32) -> Result<u64, String> {
        let cluster = u64::from(cluster);
        if cluster < 2 || cluster > self.geometry.cluster_count + 1 {
            return Err(format!("exFAT cluster out of range: {cluster}"));
        }
        let relative = (cluster - 2)
            .checked_mul(self.geometry.sectors_per_cluster)
            .ok_or("exFAT cluster offset overflow")?;
        let sector = self
            .geometry
            .heap_offset
            .checked_add(relative)
            .ok_or("exFAT cluster sector overflow")?;
        let offset = sector
            .checked_mul(self.geometry.sector_size)
            .ok_or("exFAT cluster byte offset overflow")?;
        let end = offset
            .checked_add(self.cluster_bytes()? as u64)
            .ok_or("exFAT cluster byte range overflow")?;
        let volume_bytes = self
            .geometry
            .volume_length
            .checked_mul(self.geometry.sector_size)
            .ok_or("exFAT volume byte range overflow")?;
        if end > volume_bytes {
            return Err("exFAT cluster byte range exceeds volume".to_string());
        }
        Ok(offset)
    }

    fn fat_entry(&mut self, cluster: u32) -> Result<u32, String> {
        if u64::from(cluster) > self.geometry.cluster_count + 1 {
            return Err(format!("exFAT FAT cluster out of range: {cluster}"));
        }
        let fat_byte = u64::from(cluster)
            .checked_mul(4)
            .ok_or("exFAT FAT entry offset overflow")?;
        let fat_bytes = self
            .geometry
            .fat_length
            .checked_mul(self.geometry.sector_size)
            .ok_or("exFAT FAT byte length overflow")?;
        if fat_byte.checked_add(4).is_none_or(|end| end > fat_bytes) {
            return Err("exFAT FAT entry exceeds FAT length".to_string());
        }
        let offset = self
            .geometry
            .fat_offset
            .checked_mul(self.geometry.sector_size)
            .and_then(|base| base.checked_add(fat_byte))
            .ok_or("exFAT FAT byte offset overflow")?;
        let mut value = [0_u8; 4];
        self.read(&mut value, offset, "exFAT FAT entry")?;
        Ok(u32::from_le_bytes(value))
    }

    fn read_chain(&mut self, first_cluster: u32) -> Result<Vec<u8>, String> {
        let cluster_bytes = self.cluster_bytes()?;
        let mut output = Vec::new();
        let mut cluster = first_cluster;
        let mut seen = BTreeSet::new();
        for _ in 0..MAX_CHAIN_CLUSTERS {
            if !seen.insert(cluster) {
                return Err(format!("exFAT cluster chain loop at {cluster}"));
            }
            let offset = self.cluster_offset(cluster)?;
            let next_len = output
                .len()
                .checked_add(cluster_bytes)
                .ok_or("exFAT directory output overflow")?;
            if next_len as u64 > MAX_DIRECTORY_BYTES {
                return Err("exFAT directory exceeds shadow byte budget".to_string());
            }
            let old_len = output.len();
            output.resize(next_len, 0);
            self.read(&mut output[old_len..], offset, "exFAT directory cluster")?;
            let next = self.fat_entry(cluster)?;
            if next >= LAST_CLUSTER {
                return Ok(output);
            }
            if next == 0 || next == 0xffff_fff7 || next < 2 {
                return Err(format!(
                    "invalid exFAT FAT link {next:#x} after cluster {cluster} (fat_offset={} heap_offset={} sector_size={} sectors_per_cluster={} root_cluster={} cluster_count={})",
                    self.geometry.fat_offset,
                    self.geometry.heap_offset,
                    self.geometry.sector_size,
                    self.geometry.sectors_per_cluster,
                    self.geometry.root_cluster,
                    self.geometry.cluster_count,
                ));
            }
            cluster = next;
        }
        Err("exFAT directory chain exceeds cluster budget".to_string())
    }

    fn walk_to_target(
        &mut self,
        cluster: u32,
        parent: &Path,
        components: &[String],
        depth: usize,
    ) -> Result<(), String> {
        if components.is_empty() {
            return self.walk_directory(cluster, parent, depth, false, 0);
        }
        let bytes = self.read_chain(cluster)?;
        for set in parse_file_sets(&bytes)? {
            if set.kind == ShadowEntryKind::Directory && set.name == components[0] {
                let path = parent.join(&set.name);
                return self.walk_to_target(set.first_cluster, &path, &components[1..], depth + 1);
            }
        }
        Err(format!(
            "exFAT shadow path component not found: {}",
            components[0]
        ))
    }

    fn walk_directory(
        &mut self,
        cluster: u32,
        parent: &Path,
        depth: usize,
        no_fat_chain: bool,
        data_length: u64,
    ) -> Result<(), String> {
        if depth > MAX_DIRECTORY_DEPTH {
            return Err("exFAT directory depth exceeds shadow budget".to_string());
        }
        self.directories = self.directories.saturating_add(1);
        let bytes = if no_fat_chain {
            self.read_contiguous(cluster, data_length)?
        } else {
            self.read_chain(cluster)?
        };
        for set in parse_file_sets(&bytes)? {
            if self.entries.len() >= MAX_DIRECTORY_ENTRIES {
                return Err("exFAT entry budget exceeded".to_string());
            }
            let path = parent.join(&set.name);
            self.entries.insert(
                path.clone(),
                ShadowEntry {
                    kind: set.kind,
                    size: set.size,
                    first_cluster: set.first_cluster,
                },
            );
            if set.kind == ShadowEntryKind::Directory && set.first_cluster >= 2 {
                self.walk_directory(
                    set.first_cluster,
                    &path,
                    depth + 1,
                    set.no_fat_chain,
                    set.size,
                )?;
            }
        }
        Ok(())
    }

    fn read_contiguous(&mut self, first_cluster: u32, data_length: u64) -> Result<Vec<u8>, String> {
        let cluster_bytes = self.cluster_bytes()? as u64;
        let clusters = data_length
            .saturating_add(cluster_bytes.saturating_sub(1))
            .checked_div(cluster_bytes)
            .ok_or("exFAT contiguous stream size overflow")?;
        if clusters > MAX_CHAIN_CLUSTERS as u64
            || clusters.saturating_mul(cluster_bytes) > MAX_DIRECTORY_BYTES
        {
            return Err("exFAT contiguous directory exceeds shadow budget".to_string());
        }
        let mut output = vec![0_u8; clusters as usize * cluster_bytes as usize];
        for (index, chunk) in output.chunks_exact_mut(cluster_bytes as usize).enumerate() {
            let cluster = u32::try_from(u64::from(first_cluster).saturating_add(index as u64))
                .map_err(|_| "exFAT contiguous cluster overflow".to_string())?;
            let offset = self.cluster_offset(cluster)?;
            self.read(chunk, offset, "exFAT contiguous directory cluster")?;
        }
        output.truncate(usize::try_from(data_length).unwrap_or(output.len()));
        Ok(output)
    }

    fn read(&mut self, buffer: &mut [u8], offset: u64, what: &str) -> Result<(), String> {
        read_at(&self.file, buffer, offset, what)?;
        self.requests = self.requests.saturating_add(1);
        self.bytes_read = self.bytes_read.saturating_add(buffer.len() as u64);
        Ok(())
    }
}

fn parse_file_sets(bytes: &[u8]) -> Result<Vec<FileSet>, String> {
    let mut sets = Vec::new();
    let mut offset = 0usize;
    while offset + 32 <= bytes.len() {
        let entry_type = bytes[offset];
        if entry_type == 0 {
            break;
        }
        if entry_type != FILE_ENTRY {
            offset += 32;
            continue;
        }
        let secondary_count = usize::from(bytes[offset + 1]);
        let set_bytes = (secondary_count + 1)
            .checked_mul(32)
            .ok_or("exFAT file-set length overflow")?;
        if offset + set_bytes > bytes.len() {
            return Err("truncated exFAT file entry set".to_string());
        }
        let set = &bytes[offset..offset + set_bytes];
        let attributes = u16::from_le_bytes([set[4], set[5]]);
        let kind = if attributes & 0x10 != 0 {
            ShadowEntryKind::Directory
        } else {
            ShadowEntryKind::File
        };
        let mut first_cluster = 0_u32;
        let mut size = 0_u64;
        let mut name_length = 0usize;
        let mut no_fat_chain = false;
        let mut name_units = Vec::new();
        for secondary in set[32..].chunks_exact(32) {
            match secondary[0] {
                STREAM_ENTRY => {
                    no_fat_chain = secondary[1] & 0x02 != 0;
                    name_length = usize::from(secondary[3]);
                    first_cluster = u32::from_le_bytes([
                        secondary[20],
                        secondary[21],
                        secondary[22],
                        secondary[23],
                    ]);
                    size = u64::from_le_bytes([
                        secondary[24],
                        secondary[25],
                        secondary[26],
                        secondary[27],
                        secondary[28],
                        secondary[29],
                        secondary[30],
                        secondary[31],
                    ]);
                }
                NAME_ENTRY => {
                    for unit in secondary[2..].chunks_exact(2) {
                        name_units.push(u16::from_le_bytes([unit[0], unit[1]]));
                    }
                }
                _ => {}
            }
        }
        if name_length == 0 || name_length > name_units.len() {
            return Err("invalid exFAT file-name entry".to_string());
        }
        let name = String::from_utf16(&name_units[..name_length])
            .map_err(|_| "invalid UTF-16 in exFAT file name".to_string())?;
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err("unsafe exFAT directory name".to_string());
        }
        sets.push(FileSet {
            name,
            kind,
            size,
            first_cluster,
            no_fat_chain,
        });
        offset += set_bytes;
    }
    Ok(sets)
}

fn read_at(file: &File, buffer: &mut [u8], offset: u64, what: &str) -> Result<(), String> {
    let mut done = 0usize;
    while done < buffer.len() {
        let position = offset
            .checked_add(done as u64)
            .ok_or_else(|| format!("{what} offset overflow"))?;
        let count = file
            .read_at(&mut buffer[done..], position)
            .map_err(|error| format!("{what} at {position}: {error}"))?;
        if count == 0 {
            return Err(format!("{what} ended after {done} bytes"));
        }
        done += count;
    }
    Ok(())
}

fn le_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("fixed exFAT field"),
    )
}

fn le_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("fixed exFAT field"),
    )
}

fn mounted_block_source(path: &Path) -> Result<Option<(PathBuf, PathBuf)>, String> {
    let text = match std::fs::read_to_string("/proc/self/mountinfo") {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read mountinfo: {error}")),
    };
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut best = None::<(PathBuf, PathBuf)>;
    for line in text.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let fields = before.split_whitespace().collect::<Vec<_>>();
        let post = after.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 6 || post.len() < 2 {
            continue;
        }
        let mountpoint = decode_mount_field(fields[4]);
        if !canonical.starts_with(&mountpoint) {
            continue;
        }
        let source = decode_mount_field(post[1]);
        if !source.starts_with("/dev/") {
            continue;
        }
        let source = resolve_mount_device(&mountpoint, source)?;
        if best.as_ref().is_some_and(|(existing, _)| {
            existing.components().count() >= mountpoint.components().count()
        }) {
            continue;
        }
        best = Some((mountpoint, PathBuf::from(source)));
    }
    Ok(best)
}

fn resolve_mount_device(mountpoint: &Path, source: PathBuf) -> Result<PathBuf, String> {
    if source.exists() {
        return Ok(source);
    }

    #[cfg(target_os = "linux")]
    {
        let mount_device = std::fs::metadata(mountpoint)
            .map_err(|error| format!("stat exFAT mountpoint {}: {error}", mountpoint.display()))?
            .dev();
        let directory = std::fs::read_dir("/dev")
            .map_err(|error| format!("read /dev while resolving exFAT backing device: {error}"))?;
        for entry in directory {
            let entry = entry.map_err(|error| format!("read /dev entry: {error}"))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with("mmcblk") || name.starts_with("sd")) {
                continue;
            }
            let candidate = entry.path();
            let metadata = match std::fs::metadata(&candidate) {
                Ok(metadata) if metadata.file_type().is_block_device() => metadata,
                _ => continue,
            };
            if metadata.rdev() == mount_device {
                return Ok(candidate);
            }
        }
    }

    Err(format!(
        "exFAT backing device {} is unavailable and could not be resolved from mount {}",
        source.display(),
        mountpoint.display()
    ))
}

fn decode_mount_field(value: &str) -> PathBuf {
    let mut bytes = Vec::with_capacity(value.len());
    let raw = value.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if index + 4 <= raw.len() && raw[index] == b'\\' {
            if let Ok(value) = u8::from_str_radix(&value[index + 1..index + 4], 8) {
                bytes.push(value);
                index += 4;
                continue;
            }
        }
        bytes.push(raw[index]);
        index += 1;
    }
    PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_set(name: &str, directory: bool, first_cluster: u32, size: u64) -> Vec<u8> {
        let units = name.encode_utf16().collect::<Vec<_>>();
        let name_entries = units.len().div_ceil(15);
        let secondary_count = 1 + name_entries;
        let mut bytes = vec![0_u8; (secondary_count + 1) * 32];
        bytes[0] = FILE_ENTRY;
        bytes[1] = u8::try_from(secondary_count).expect("test entry set fits");
        let attributes: u16 = if directory { 0x10 } else { 0 };
        bytes[4..6].copy_from_slice(&attributes.to_le_bytes());
        bytes[32] = STREAM_ENTRY;
        bytes[35] = u8::try_from(units.len()).expect("test name fits");
        bytes[52..56].copy_from_slice(&first_cluster.to_le_bytes());
        bytes[56..64].copy_from_slice(&size.to_le_bytes());
        for (index, chunk) in units.chunks(15).enumerate() {
            let offset = 64 + index * 32;
            bytes[offset] = NAME_ENTRY;
            for (unit_index, unit) in chunk.iter().enumerate() {
                bytes[offset + 2 + unit_index * 2..offset + 4 + unit_index * 2]
                    .copy_from_slice(&unit.to_le_bytes());
            }
        }
        bytes
    }

    #[test]
    fn parses_file_and_directory_sets() {
        let mut bytes = file_set("hello.sfc", false, 42, 1234);
        bytes.extend(file_set("nested", true, 43, 4096));
        let sets = parse_file_sets(&bytes).expect("valid exFAT file sets");
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0].name, "hello.sfc");
        assert_eq!(sets[0].kind, ShadowEntryKind::File);
        assert_eq!(sets[0].first_cluster, 42);
        assert_eq!(sets[0].size, 1234);
        assert_eq!(sets[1].name, "nested");
        assert_eq!(sets[1].kind, ShadowEntryKind::Directory);
    }

    #[test]
    fn rejects_truncated_file_set() {
        let mut bytes = file_set("broken", false, 2, 1);
        bytes.truncate(40);
        assert!(parse_file_sets(&bytes).is_err());
    }

    #[test]
    fn decodes_mount_escapes() {
        assert_eq!(
            decode_mount_field(r"/media/My\040Card"),
            PathBuf::from("/media/My Card")
        );
        assert_eq!(
            decode_mount_field(r"/dev/mmcblk0p1"),
            PathBuf::from("/dev/mmcblk0p1")
        );
    }
}
