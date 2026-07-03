use std::fs::File;
use std::io::Read;

pub(super) const V2_PIXELS_MAGIC: &[u8; 8] = b"MMPX2B1\0";
pub(super) const SIDECAR_INDEX_MAGIC: &[u8; 8] = b"MMIDX02\0";
pub(super) const MAX_PREVIEW_ARCHIVE_ENTRIES: usize = 100_000;
pub(super) const MAX_PREVIEW_ARCHIVE_RAW_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PreviewArchiveEntry {
    pub(super) raw_len: usize,
    pub(super) compressed_len: usize,
    pub(super) offset: u64,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) stride_bytes: u32,
    pub(super) payload_flag: u8,
}

pub(super) fn read_file_entry(
    file: &mut File,
    context: &str,
    archive_bytes: u64,
) -> Result<(String, PreviewArchiveEntry), String> {
    let name_len = read_u16(file)? as usize;
    let width = read_u32(file)?;
    let height = read_u32(file)?;
    let stride_bytes = read_u32(file)?;
    let raw_len = read_u32(file)? as usize;
    let mut payload_flag = [0u8; 1];
    file.read_exact(&mut payload_flag)
        .map_err(|e| format!("read {context} payload flag: {e}"))?;
    let compressed_len = read_u32(file)? as usize;
    let offset = read_u64(file)?;
    let mut name = vec![0u8; name_len];
    file.read_exact(&mut name)
        .map_err(|e| format!("read {context} entry name: {e}"))?;
    let name = String::from_utf8(name).map_err(|e| format!("{context} entry name utf8: {e}"))?;
    let entry = validate_entry(
        context,
        &name,
        width,
        height,
        stride_bytes,
        raw_len,
        payload_flag[0],
        compressed_len,
        offset,
        archive_bytes,
    )?;
    Ok((name, entry))
}

pub(super) fn read_sidecar_entry(
    bytes: &[u8],
    pos: &mut usize,
    context: &str,
    archive_bytes: u64,
) -> Result<(String, PreviewArchiveEntry), String> {
    if bytes.len().saturating_sub(*pos) < 31 {
        return Err(format!("{context}: truncated preview archive index"));
    }
    let name_len = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap()) as usize;
    *pos += 2;
    let width = u32::from_le_bytes(bytes[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    let height = u32::from_le_bytes(bytes[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    let stride_bytes = u32::from_le_bytes(bytes[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    let raw_len = u32::from_le_bytes(bytes[*pos..*pos + 4].try_into().unwrap()) as usize;
    *pos += 4;
    let payload_flag = bytes[*pos];
    *pos += 1;
    let compressed_len = u32::from_le_bytes(bytes[*pos..*pos + 4].try_into().unwrap()) as usize;
    *pos += 4;
    let offset = u64::from_le_bytes(bytes[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    let end = (*pos)
        .checked_add(name_len)
        .ok_or_else(|| format!("{context}: preview archive index name overflow"))?;
    if end > bytes.len() {
        return Err(format!("{context}: truncated preview archive index name"));
    }
    let name = String::from_utf8(bytes[*pos..end].to_vec())
        .map_err(|e| format!("preview archive index name utf8: {e}"))?;
    *pos = end;
    let entry = validate_entry(
        context,
        &name,
        width,
        height,
        stride_bytes,
        raw_len,
        payload_flag,
        compressed_len,
        offset,
        archive_bytes,
    )?;
    Ok((name, entry))
}

#[allow(clippy::too_many_arguments)]
fn validate_entry(
    context: &str,
    name: &str,
    width: u32,
    height: u32,
    stride_bytes: u32,
    raw_len: usize,
    payload_flag: u8,
    compressed_len: usize,
    offset: u64,
    archive_bytes: u64,
) -> Result<PreviewArchiveEntry, String> {
    validate_entry_geometry(context, name, width, height, stride_bytes, raw_len)?;
    validate_entry_payload(context, name, payload_flag, raw_len, compressed_len)?;
    validate_entry_bounds(context, name, offset, compressed_len, archive_bytes)?;
    Ok(PreviewArchiveEntry {
        raw_len,
        compressed_len,
        offset,
        width,
        height,
        stride_bytes,
        payload_flag,
    })
}

pub(super) fn validate_entry_count(count: usize, context: &str) -> Result<(), String> {
    if count > MAX_PREVIEW_ARCHIVE_ENTRIES {
        return Err(format!(
            "{context} has {count} entries, max {MAX_PREVIEW_ARCHIVE_ENTRIES}"
        ));
    }
    Ok(())
}

pub(super) fn validate_entry_geometry(
    context: &str,
    name: &str,
    width: u32,
    height: u32,
    stride_bytes: u32,
    raw_len: usize,
) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err(format!(
            "{context} bad geometry for {name}: width={width} height={height}"
        ));
    }
    let width_bytes = (width as usize)
        .checked_mul(2)
        .ok_or_else(|| format!("{context} width overflow for {name}: width={width}"))?;
    let stride = stride_bytes as usize;
    if !stride.is_multiple_of(2) || stride < width_bytes {
        return Err(format!(
            "{context} bad stride for {name}: width={width} stride={stride_bytes}"
        ));
    }
    let expected = stride
        .checked_mul(height as usize)
        .ok_or_else(|| format!("{context} raw length overflow for {name}"))?;
    if expected > MAX_PREVIEW_ARCHIVE_RAW_BYTES {
        return Err(format!(
            "{context} raw length too large for {name}: {expected} > {MAX_PREVIEW_ARCHIVE_RAW_BYTES}"
        ));
    }
    if raw_len != expected {
        return Err(format!(
            "{context} bad geometry for {name}: width={width} height={height} stride={stride_bytes} raw_len={raw_len} expected={expected}"
        ));
    }
    Ok(expected)
}

fn validate_entry_payload(
    context: &str,
    name: &str,
    payload_flag: u8,
    raw_len: usize,
    compressed_len: usize,
) -> Result<(), String> {
    let max_encoded = match payload_flag {
        0 => raw_len
            .checked_add(raw_len / 255)
            .and_then(|n| n.checked_add(16))
            .ok_or_else(|| format!("{context} compressed length overflow for {name}"))?,
        1 => {
            if compressed_len != raw_len {
                return Err(format!(
                    "{context} raw payload length mismatch for {name}: compressed_len={compressed_len} raw_len={raw_len}"
                ));
            }
            raw_len
        }
        other => {
            return Err(format!(
                "{context} unsupported payload flag {other} for {name}"
            ));
        }
    };
    if compressed_len == 0 || compressed_len > max_encoded {
        return Err(format!(
            "{context} encoded length too large for {name}: {compressed_len} > {max_encoded}"
        ));
    }
    Ok(())
}

fn validate_entry_bounds(
    context: &str,
    name: &str,
    offset: u64,
    compressed_len: usize,
    archive_bytes: u64,
) -> Result<(), String> {
    let payload_end = offset
        .checked_add(compressed_len as u64)
        .ok_or_else(|| format!("{context} offset overflow for {name}"))?;
    if payload_end > archive_bytes {
        return Err(format!(
            "{context} payload outside archive for {name}: end={payload_end} archive_bytes={archive_bytes}"
        ));
    }
    Ok(())
}

pub(super) fn read_u16(file: &mut File) -> Result<u16, String> {
    let mut buf = [0u8; 2];
    file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(u16::from_le_bytes(buf))
}

pub(super) fn read_u32(file: &mut File) -> Result<u32, String> {
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(file: &mut File) -> Result<u64, String> {
    let mut buf = [0u8; 8];
    file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(u64::from_le_bytes(buf))
}
