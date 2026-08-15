// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const MAGIC_V1: &[u8; 8] = b"MMLZ4B1\0";
const MAGIC_V2_PIXELS: &[u8; 8] = b"MMPX2B1\0";
const HEADER: &str = "preview_pack_bench_tsv\tlabel\tvariant\tcodec\titeration\tordinal\tasset_key\toffset\tentry_flag\tencoded_bytes\tdecoded_bytes\tcompression_ratio\twidth\theight\tload_source\tindex_lookup_us\tread_us\tdecode_us\traw565_parse_us\ttotal_us\tdecode_mb_s\ttotal_mb_s\tchecksum32\tresult\terror\tdecode_cpu_us\traw565_parse_cpu_us";

#[derive(Clone, Debug)]
struct Config {
    label: String,
    variant: String,
    codec: String,
    pack: PathBuf,
    iterations: usize,
    order: Order,
    warm: WarmMode,
    sample: Sample,
    pack_sizes: Vec<PackSizeSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Order {
    Sequential,
    Random,
    CatalogScroll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WarmMode {
    Full,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sample {
    All,
    Count(usize),
}

#[derive(Clone, Debug)]
struct PackSizeSpec {
    system: String,
    path: PathBuf,
}

#[derive(Clone, Debug)]
struct Archive {
    bytes: Vec<u8>,
    entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    raw_len: usize,
    payload_len: usize,
    offset: usize,
    format: EntryFormat,
}

#[derive(Clone, Debug)]
enum EntryFormat {
    V1Raw565,
    V2Pixels {
        width: u32,
        height: u32,
        stride_bytes: u32,
        payload_flag: u8,
    },
}

#[derive(Clone, Debug)]
struct Row {
    label: String,
    variant: String,
    codec: String,
    iteration: usize,
    ordinal: usize,
    asset_key: String,
    offset: usize,
    entry_flag: String,
    encoded_bytes: usize,
    decoded_bytes: usize,
    compression_ratio: f64,
    width: u32,
    height: u32,
    load_source: String,
    index_lookup_us: u64,
    read_us: u64,
    decode_us: u64,
    raw565_parse_us: u64,
    decode_cpu_us: u64,
    raw565_parse_cpu_us: u64,
    total_us: u64,
    decode_mb_s: f64,
    total_mb_s: f64,
    checksum32: u32,
    result: String,
    error: String,
}

pub fn run() {
    match run_inner(std::env::args().skip(2)) {
        Ok(()) => {}
        Err(error) => {
            crate::ui_errln!("preview-pack-bench failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run_inner<I>(args: I) -> Result<(), String>
where
    I: IntoIterator<Item = String>,
{
    let config = parse_args(args)?;
    print_device_meta();
    print_binary_meta();
    print_pack_sizes(&config)?;

    let warm_t = Instant::now();
    let archive = read_archive(&config.pack)?;
    let warm_us = warm_t.elapsed().as_micros() as u64;
    crate::ui_logln!(
        "warm_meta\tmode={}\telapsed_us={}\tloaded=1\tpack_bytes={}",
        config.warm.label(),
        warm_us,
        archive.bytes.len()
    );
    crate::ui_logln!("{HEADER}");

    let order = ordered_indices(&archive.entries, config.order, config.sample);
    let mut scratch = Vec::new();
    if let Some(max_raw_len) = archive.entries.iter().map(|entry| entry.raw_len).max() {
        scratch.resize(max_raw_len, 0);
    }
    for iteration in 1..=config.iterations {
        for (ordinal, entry_index) in order.iter().copied().enumerate() {
            let entry = &archive.entries[entry_index];
            let row = decode_row(
                &config,
                &archive.bytes,
                entry,
                iteration,
                ordinal + 1,
                &mut scratch,
            );
            crate::ui_logln!("{}", row.to_tsv());
            if row.result != "ok" {
                return Err(row.error);
            }
        }
    }
    Ok(())
}

fn parse_args<I>(args: I) -> Result<Config, String>
where
    I: IntoIterator<Item = String>,
{
    let mut config = Config {
        label: default_label(),
        variant: "mmlz4b-lz4-fast".to_string(),
        codec: "lz4-flex".to_string(),
        pack: mister_magik_catalog::device_layout::current_app_path(
            "assets/arcade-screenshots-320x320.mmlz4b",
        ),
        iterations: 5,
        order: Order::Random,
        warm: WarmMode::Full,
        sample: Sample::All,
        pack_sizes: Vec::new(),
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--label" => config.label = args.next().ok_or("--label requires a value")?,
            "--variant" => config.variant = args.next().ok_or("--variant requires a value")?,
            "--codec" => config.codec = args.next().ok_or("--codec requires a value")?,
            "--pack" => config.pack = PathBuf::from(args.next().ok_or("--pack requires a path")?),
            "--iterations" => {
                config.iterations = args
                    .next()
                    .ok_or("--iterations requires a value")?
                    .parse::<usize>()
                    .map_err(|e| format!("invalid --iterations: {e}"))?;
            }
            "--order" => {
                config.order = Order::parse(&args.next().ok_or("--order requires a value")?)?;
            }
            "--warm" => {
                config.warm = WarmMode::parse(&args.next().ok_or("--warm requires a value")?)?;
            }
            "--cache" => {
                let value = args.next().ok_or("--cache requires a value")?;
                if value != "decoded-off" {
                    return Err("preview-pack-bench only supports --cache decoded-off".to_string());
                }
            }
            "--sample" => {
                let value = args.next().ok_or("--sample requires a value")?;
                config.sample = Sample::parse(&value)?;
            }
            "--pack-size" => {
                let value = args.next().ok_or("--pack-size requires system=path")?;
                config.pack_sizes.push(PackSizeSpec::parse(&value)?);
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if config.iterations == 0 {
        return Err("--iterations must be greater than zero".to_string());
    }
    if config.warm != WarmMode::Full {
        return Err("codec comparison requires --warm full".to_string());
    }
    Ok(config)
}

fn print_usage() {
    crate::ui_logln!(
        "usage: mister-magik-fb preview-pack-bench --pack PATH [--label LABEL] [--variant NAME] [--iterations N] [--order sequential|random|catalog-scroll] [--warm full] [--cache decoded-off] [--sample all|N] [--pack-size system=PATH]..."
    );
}

impl Order {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "sequential" => Ok(Self::Sequential),
            "random" => Ok(Self::Random),
            "catalog-scroll" => Ok(Self::CatalogScroll),
            other => Err(format!("unsupported --order: {other}")),
        }
    }
}

impl WarmMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "full" => Ok(Self::Full),
            "none" => Ok(Self::None),
            other => Err(format!("unsupported --warm: {other}")),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::None => "none",
        }
    }
}

impl Sample {
    fn parse(value: &str) -> Result<Self, String> {
        if value == "all" {
            return Ok(Self::All);
        }
        let count = value
            .parse::<usize>()
            .map_err(|e| format!("invalid --sample: {e}"))?;
        if count == 0 {
            return Err("--sample must be all or a positive count".to_string());
        }
        Ok(Self::Count(count))
    }
}

impl PackSizeSpec {
    fn parse(value: &str) -> Result<Self, String> {
        let (system, path) = value
            .split_once('=')
            .ok_or("--pack-size must be system=path")?;
        if system.is_empty() || path.is_empty() {
            return Err("--pack-size must be system=path".to_string());
        }
        Ok(Self {
            system: system.to_string(),
            path: PathBuf::from(path),
        })
    }
}

impl Row {
    fn to_tsv(&self) -> String {
        format!(
            "preview_pack_bench_tsv\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.2}\t{:.2}\t{:08x}\t{}\t{}\t{}\t{}",
            tsv(&self.label),
            tsv(&self.variant),
            tsv(&self.codec),
            self.iteration,
            self.ordinal,
            tsv(&self.asset_key),
            self.offset,
            self.entry_flag,
            self.encoded_bytes,
            self.decoded_bytes,
            self.compression_ratio,
            self.width,
            self.height,
            self.load_source,
            self.index_lookup_us,
            self.read_us,
            self.decode_us,
            self.raw565_parse_us,
            self.total_us,
            self.decode_mb_s,
            self.total_mb_s,
            self.checksum32,
            self.result,
            tsv(&self.error),
            self.decode_cpu_us,
            self.raw565_parse_cpu_us
        )
    }
}

fn read_archive(path: &Path) -> Result<Archive, String> {
    let bytes = fs::read(path).map_err(|e| format!("read archive {}: {e}", path.display()))?;
    let entries = parse_entries(&bytes)?;
    Ok(Archive { bytes, entries })
}

fn parse_entries(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    if bytes.len() < 12 {
        return Err("preview archive is too short".to_string());
    }
    if &bytes[..8] == MAGIC_V1 {
        return parse_v1_entries(bytes);
    }
    if &bytes[..8] == MAGIC_V2_PIXELS {
        return parse_v2_pixels_entries(bytes);
    }
    Err("preview archive has bad magic".to_string())
}

fn parse_v1_entries(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let count = read_u32(bytes, 8)? as usize;
    let mut pos = 12usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = read_u16(bytes, pos)? as usize;
        pos += 2;
        let raw_len = read_u32(bytes, pos)? as usize;
        pos += 4;
        let payload_len = read_u32(bytes, pos)? as usize;
        pos += 4;
        let offset = read_u64(bytes, pos)? as usize;
        pos += 8;
        let name_end = pos
            .checked_add(name_len)
            .ok_or("preview archive name offset overflow")?;
        let name = bytes
            .get(pos..name_end)
            .ok_or("preview archive entry name is truncated")
            .and_then(|name| std::str::from_utf8(name).map_err(|_| "entry name is not utf-8"))?
            .to_string();
        pos = name_end;
        let end = offset
            .checked_add(payload_len)
            .ok_or("preview archive payload offset overflow")?;
        if end > bytes.len() {
            return Err(format!("preview archive payload out of range: {name}"));
        }
        entries.push(Entry {
            name,
            raw_len,
            payload_len,
            offset,
            format: EntryFormat::V1Raw565,
        });
    }
    Ok(entries)
}

fn parse_v2_pixels_entries(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let count = read_u32(bytes, 8)? as usize;
    let mut pos = 12usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = read_u16(bytes, pos)? as usize;
        pos += 2;
        let width = read_u32(bytes, pos)?;
        pos += 4;
        let height = read_u32(bytes, pos)?;
        pos += 4;
        let stride_bytes = read_u32(bytes, pos)?;
        pos += 4;
        let decoded_pixel_bytes = read_u32(bytes, pos)? as usize;
        pos += 4;
        let payload_flag = *bytes
            .get(pos)
            .ok_or("preview archive v2 payload flag is truncated")?;
        pos += 1;
        let payload_len = read_u32(bytes, pos)? as usize;
        pos += 4;
        let offset = read_u64(bytes, pos)? as usize;
        pos += 8;
        let name_end = pos
            .checked_add(name_len)
            .ok_or("preview archive v2 name offset overflow")?;
        let name = bytes
            .get(pos..name_end)
            .ok_or("preview archive v2 entry name is truncated")
            .and_then(|name| std::str::from_utf8(name).map_err(|_| "entry name is not utf-8"))?
            .to_string();
        pos = name_end;
        if stride_bytes < width.saturating_mul(2) {
            return Err(format!(
                "preview archive v2 bad stride for {name}: width={width} stride={stride_bytes}"
            ));
        }
        let expected_pixel_bytes = (stride_bytes as usize)
            .checked_mul(height as usize)
            .ok_or("preview archive v2 decoded byte length overflow")?;
        if decoded_pixel_bytes != expected_pixel_bytes {
            return Err(format!(
                "preview archive v2 decoded length got={decoded_pixel_bytes} expected={expected_pixel_bytes}: {name}"
            ));
        }
        if !matches!(payload_flag, 0 | 1) {
            return Err(format!(
                "preview archive v2 unsupported payload flag {payload_flag}: {name}"
            ));
        }
        let end = offset
            .checked_add(payload_len)
            .ok_or("preview archive v2 payload offset overflow")?;
        if end > bytes.len() {
            return Err(format!("preview archive v2 payload out of range: {name}"));
        }
        entries.push(Entry {
            name,
            raw_len: decoded_pixel_bytes,
            payload_len,
            offset,
            format: EntryFormat::V2Pixels {
                width,
                height,
                stride_bytes,
                payload_flag,
            },
        });
    }
    Ok(entries)
}

fn decode_row(
    config: &Config,
    bytes: &[u8],
    entry: &Entry,
    iteration: usize,
    ordinal: usize,
    scratch: &mut Vec<u8>,
) -> Row {
    let total_t = Instant::now();
    let read_t = Instant::now();
    let payload = &bytes[entry.offset..entry.offset + entry.payload_len];
    let read_us = read_t.elapsed().as_micros() as u64;

    let decode_t = Instant::now();
    let decode_cpu_t = thread_cpu_us();
    let decoded = decode_payload(payload, entry, scratch);
    let decode_us = decode_t.elapsed().as_micros() as u64;
    let decode_cpu_us = elapsed_thread_cpu_us(decode_cpu_t);
    match decoded {
        Ok((entry_flag, data)) => {
            let parse_t = Instant::now();
            let parse_cpu_t = thread_cpu_us();
            let parsed = match entry.format {
                EntryFormat::V1Raw565 => parse_raw565(data),
                EntryFormat::V2Pixels {
                    width,
                    height,
                    stride_bytes,
                    ..
                } => parse_pixels(width, height, stride_bytes, data),
            };
            match parsed {
                Ok((width, height, decoded_bytes, checksum32)) => {
                    let raw565_parse_us = parse_t.elapsed().as_micros() as u64;
                    let raw565_parse_cpu_us = elapsed_thread_cpu_us(parse_cpu_t);
                    let total_us = total_t.elapsed().as_micros() as u64;
                    let decoded_mb = decoded_bytes as f64 / (1024.0 * 1024.0);
                    Row {
                        label: config.label.clone(),
                        variant: config.variant.clone(),
                        codec: config.codec.clone(),
                        iteration,
                        ordinal,
                        asset_key: asset_key(&entry.name),
                        offset: entry.offset,
                        entry_flag: entry_flag.to_string(),
                        encoded_bytes: entry.payload_len,
                        decoded_bytes,
                        compression_ratio: entry.payload_len as f64 / decoded_bytes as f64,
                        width,
                        height,
                        load_source: "archive_mem".to_string(),
                        index_lookup_us: 0,
                        read_us,
                        decode_us,
                        raw565_parse_us,
                        decode_cpu_us,
                        raw565_parse_cpu_us,
                        total_us,
                        decode_mb_s: mb_per_sec(decoded_mb, decode_us),
                        total_mb_s: mb_per_sec(decoded_mb, total_us),
                        checksum32,
                        result: "ok".to_string(),
                        error: String::new(),
                    }
                }
                Err(error) => error_row(
                    config,
                    entry,
                    iteration,
                    ordinal,
                    read_us,
                    decode_us,
                    decode_cpu_us,
                    error,
                ),
            }
        }
        Err(error) => error_row(
            config,
            entry,
            iteration,
            ordinal,
            read_us,
            decode_us,
            decode_cpu_us,
            error,
        ),
    }
}

fn decode_payload<'a>(
    payload: &'a [u8],
    entry: &Entry,
    scratch: &'a mut Vec<u8>,
) -> Result<(&'static str, &'a [u8]), String> {
    let (flag, block) = match entry.format {
        EntryFormat::V1Raw565 => {
            let (&flag, block) = payload
                .split_first()
                .ok_or_else(|| "empty preview archive payload".to_string())?;
            (flag, block)
        }
        EntryFormat::V2Pixels { payload_flag, .. } => (payload_flag, payload),
    };
    match flag {
        0 => {
            if scratch.len() < entry.raw_len {
                scratch.resize(entry.raw_len, 0);
            }
            let len = lz4_flex::block::decompress_into(block, &mut scratch[..entry.raw_len])
                .map_err(|e| format!("lz4 decode: {e}"))?;
            if len != entry.raw_len {
                return Err(format!(
                    "lz4 decoded length got={len} expected={}",
                    entry.raw_len
                ));
            }
            let label = match entry.format {
                EntryFormat::V1Raw565 => "lz4_block",
                EntryFormat::V2Pixels { .. } => "lz4_pixels",
            };
            Ok((label, &scratch[..len]))
        }
        1 => {
            if block.len() != entry.raw_len {
                return Err(format!(
                    "raw stored length got={} expected={}",
                    block.len(),
                    entry.raw_len
                ));
            }
            let label = match entry.format {
                EntryFormat::V1Raw565 => "raw_stored",
                EntryFormat::V2Pixels { .. } => "raw_pixels",
            };
            Ok((label, block))
        }
        other => Err(format!("unsupported entry flag: {other}")),
    }
}

fn parse_raw565(data: &[u8]) -> Result<(u32, u32, usize, u32), String> {
    if data.len() < 20 || &data[..8] != b"MM56501\0" {
        return Err("raw565 preview bad header".to_string());
    }
    let width = read_u32(data, 8)?;
    let height = read_u32(data, 12)?;
    let stride = read_u32(data, 16)? as usize;
    let decoded_bytes = stride
        .checked_mul(height as usize)
        .ok_or("raw565 decoded byte length overflow")?;
    if data.len() != 20 + decoded_bytes {
        return Err(format!(
            "raw565 length got={} expected={}",
            data.len(),
            20 + decoded_bytes
        ));
    }
    Ok((width, height, decoded_bytes, checksum32(data)))
}

fn parse_pixels(
    width: u32,
    height: u32,
    stride_bytes: u32,
    data: &[u8],
) -> Result<(u32, u32, usize, u32), String> {
    let decoded_bytes = (stride_bytes as usize)
        .checked_mul(height as usize)
        .ok_or("pixel decoded byte length overflow")?;
    if stride_bytes < width.saturating_mul(2) {
        return Err(format!(
            "pixel stride too small width={width} stride={stride_bytes}"
        ));
    }
    if data.len() != decoded_bytes {
        return Err(format!(
            "pixel length got={} expected={decoded_bytes}",
            data.len()
        ));
    }
    Ok((width, height, decoded_bytes, checksum32(data)))
}

fn error_row(
    config: &Config,
    entry: &Entry,
    iteration: usize,
    ordinal: usize,
    read_us: u64,
    decode_us: u64,
    decode_cpu_us: u64,
    error: String,
) -> Row {
    Row {
        label: config.label.clone(),
        variant: config.variant.clone(),
        codec: config.codec.clone(),
        iteration,
        ordinal,
        asset_key: asset_key(&entry.name),
        offset: entry.offset,
        entry_flag: "error".to_string(),
        encoded_bytes: entry.payload_len,
        decoded_bytes: entry.raw_len,
        compression_ratio: if entry.raw_len == 0 {
            0.0
        } else {
            entry.payload_len as f64 / entry.raw_len as f64
        },
        width: 0,
        height: 0,
        load_source: "archive_mem".to_string(),
        index_lookup_us: 0,
        read_us,
        decode_us,
        raw565_parse_us: 0,
        decode_cpu_us,
        raw565_parse_cpu_us: 0,
        total_us: read_us + decode_us,
        decode_mb_s: 0.0,
        total_mb_s: 0.0,
        checksum32: 0,
        result: "error".to_string(),
        error,
    }
}

fn ordered_indices(entries: &[Entry], order: Order, sample: Sample) -> Vec<usize> {
    let mut indices = (0..entries.len()).collect::<Vec<_>>();
    match order {
        Order::Sequential | Order::CatalogScroll => {}
        Order::Random => indices.sort_by_key(|idx| stable_hash(entries[*idx].name.as_bytes())),
    }
    if let Sample::Count(count) = sample {
        indices.truncate(count.min(indices.len()));
    }
    indices
}

fn print_pack_sizes(config: &Config) -> Result<(), String> {
    let mut specs = config.pack_sizes.clone();
    if !specs.iter().any(|spec| spec.system == "arcade") {
        specs.push(PackSizeSpec {
            system: "arcade".to_string(),
            path: config.pack.clone(),
        });
    }
    for spec in specs {
        match read_archive(&spec.path) {
            Ok(archive) => {
                let raw_bytes = archive
                    .entries
                    .iter()
                    .map(|entry| entry.raw_len as u64)
                    .sum::<u64>();
                let bytes = archive.bytes.len() as u64;
                let ratio = if raw_bytes == 0 {
                    0.0
                } else {
                    bytes as f64 / raw_bytes as f64
                };
                crate::ui_logln!(
                    "pack_size_tsv\tvariant={}\tsystem={}\tbytes={}\tentries={}\traw_bytes={}\tratio={:.6}\tpath={}",
                    tsv(&config.variant),
                    tsv(&spec.system),
                    bytes,
                    archive.entries.len(),
                    raw_bytes,
                    ratio,
                    tsv(&spec.path.display().to_string())
                );
            }
            Err(error) => {
                crate::ui_logln!(
                    "pack_size_tsv\tvariant={}\tsystem={}\tbytes=0\tentries=0\traw_bytes=0\tratio=0\tpath={}\terror={}",
                    tsv(&config.variant),
                    tsv(&spec.system),
                    tsv(&spec.path.display().to_string()),
                    tsv(&error)
                );
            }
        }
    }
    Ok(())
}

fn print_device_meta() {
    let cpu_model = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("Hardware") || line.starts_with("model name"))
                .and_then(|line| {
                    line.split_once(':')
                        .map(|(_, value)| value.trim().to_string())
                })
        })
        .unwrap_or_else(|| "unknown".to_string());
    let mem_available = fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("MemAvailable:"))
                .and_then(|line| line.split_whitespace().nth(1).map(str::to_string))
        })
        .unwrap_or_else(|| "unknown".to_string());
    let loadavg = fs::read_to_string("/proc/loadavg")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    crate::ui_logln!(
        "device_meta\tarch={}\tcpu_model={}\tmem_available_kb={}\tgovernor=unknown\tloadavg={}",
        std::env::consts::ARCH,
        tsv(&cpu_model),
        mem_available,
        tsv(&loadavg)
    );
}

fn print_binary_meta() {
    let binary = std::env::current_exe()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    crate::ui_logln!(
        "binary_meta\tgit_sha={}\tbinary_sha256={}\tbuild_profile={}\tbinary={}",
        option_env!("GIT_HASH").unwrap_or("unknown"),
        "unknown",
        option_env!("PROFILE").unwrap_or("unknown"),
        tsv(&binary)
    );
}

fn read_u16(bytes: &[u8], pos: usize) -> Result<u16, String> {
    let slice = bytes
        .get(pos..pos + 2)
        .ok_or_else(|| format!("short read u16 at {pos}"))?;
    Ok(u16::from_le_bytes(slice.try_into().unwrap()))
}

fn read_u32(bytes: &[u8], pos: usize) -> Result<u32, String> {
    let slice = bytes
        .get(pos..pos + 4)
        .ok_or_else(|| format!("short read u32 at {pos}"))?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

fn read_u64(bytes: &[u8], pos: usize) -> Result<u64, String> {
    let slice = bytes
        .get(pos..pos + 8)
        .ok_or_else(|| format!("short read u64 at {pos}"))?;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

fn checksum32(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |sum, byte| {
        sum.rotate_left(5).wrapping_add(u32::from(*byte))
    })
}

fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        hash ^ u64::from(*byte).wrapping_mul(0x100000001b3)
    })
}

fn asset_key(name: &str) -> String {
    name.strip_suffix(".rgb565").unwrap_or(name).to_string()
}

fn mb_per_sec(mb: f64, us: u64) -> f64 {
    if us == 0 {
        0.0
    } else {
        mb / (us as f64 / 1_000_000.0)
    }
}

#[cfg(target_os = "linux")]
fn thread_cpu_us() -> Option<u64> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts points to initialized writable storage for the duration of the
    // syscall; failures are handled by returning None.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    if rc == 0 {
        Some(ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1_000)
    } else {
        None
    }
}

#[cfg(not(target_os = "linux"))]
fn thread_cpu_us() -> Option<u64> {
    None
}

fn elapsed_thread_cpu_us(start: Option<u64>) -> u64 {
    start
        .and_then(|start| thread_cpu_us().map(|end| end.saturating_sub(start)))
        .unwrap_or(0)
}

fn default_label() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("preview-pack-bench-{secs}")
}

fn tsv(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_raw_stored_payload() {
        let raw = raw565_fixture();
        let mut payload = vec![1];
        payload.extend_from_slice(&raw);
        let mut scratch = Vec::new();
        let entry = test_entry(
            "raw.rgb565",
            raw.len(),
            payload.len(),
            EntryFormat::V1Raw565,
        );
        let (flag, decoded) = decode_payload(&payload, &entry, &mut scratch).unwrap();
        assert_eq!(flag, "raw_stored");
        assert_eq!(decoded, raw.as_slice());
        let (width, height, decoded_bytes, _) = parse_raw565(decoded).unwrap();
        assert_eq!((width, height, decoded_bytes), (2, 2, 8));
    }

    #[test]
    fn lz4_decode_scratch_grows_without_shrinking() {
        let large = raw565_fixture_with_pixels(
            4,
            2,
            &[
                0xf800, 0x07e0, 0x001f, 0xffff, 0x0000, 0x1111, 0x2222, 0x3333,
            ],
        );
        let small = raw565_fixture_with_pixels(1, 1, &[0x07e0]);
        let mut large_payload = vec![0];
        large_payload.extend_from_slice(&lz4_flex::block::compress(&large));
        let mut small_payload = vec![0];
        small_payload.extend_from_slice(&lz4_flex::block::compress(&small));
        let mut scratch = Vec::new();
        let large_entry = test_entry(
            "large.rgb565",
            large.len(),
            large_payload.len(),
            EntryFormat::V1Raw565,
        );
        let small_entry = test_entry(
            "small.rgb565",
            small.len(),
            small_payload.len(),
            EntryFormat::V1Raw565,
        );

        let (large_flag, decoded_large) =
            decode_payload(&large_payload, &large_entry, &mut scratch).unwrap();
        assert_eq!(large_flag, "lz4_block");
        assert_eq!(decoded_large, large.as_slice());
        let grown_len = scratch.len();
        assert_eq!(grown_len, large.len());

        let (small_flag, decoded_small) =
            decode_payload(&small_payload, &small_entry, &mut scratch).unwrap();
        assert_eq!(small_flag, "lz4_block");
        assert_eq!(decoded_small, small.as_slice());
        assert_eq!(scratch.len(), grown_len);
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(parse_entries(b"not-pack").is_err());
    }

    #[test]
    fn decodes_v2_pixels_payload() {
        let pixels = [0xf8, 0x00, 0x07, 0xe0, 0x00, 0x1f, 0xff, 0xff];
        let compressed = lz4_flex::block::compress(&pixels);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC_V2_PIXELS);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&"tiny.rgb565".len().to_le_bytes()[..2]);
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        let offset_pos = bytes.len();
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(b"tiny.rgb565");
        let offset = bytes.len() as u64;
        bytes[offset_pos..offset_pos + 8].copy_from_slice(&offset.to_le_bytes());
        bytes.extend_from_slice(&compressed);

        let entries = parse_entries(&bytes).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.raw_len, pixels.len());
        let mut scratch = Vec::new();
        let payload = &bytes[entry.offset..entry.offset + entry.payload_len];
        let (flag, decoded) = decode_payload(payload, entry, &mut scratch).unwrap();
        assert_eq!(flag, "lz4_pixels");
        assert_eq!(decoded, pixels.as_slice());
        let (width, height, decoded_bytes, checksum) = match entry.format {
            EntryFormat::V2Pixels {
                width,
                height,
                stride_bytes,
                ..
            } => parse_pixels(width, height, stride_bytes, decoded).unwrap(),
            EntryFormat::V1Raw565 => panic!("expected v2 pixels"),
        };
        assert_eq!((width, height, decoded_bytes), (2, 2, pixels.len()));
        assert_eq!(checksum, checksum32(&pixels));
    }

    #[test]
    fn pack_size_spec_requires_system_and_path() {
        let spec = PackSizeSpec::parse("arcade=/tmp/a.mmlz4b").unwrap();
        assert_eq!(spec.system, "arcade");
        assert_eq!(spec.path, PathBuf::from("/tmp/a.mmlz4b"));
        assert!(PackSizeSpec::parse("arcade").is_err());
    }

    fn raw565_fixture() -> Vec<u8> {
        raw565_fixture_with_pixels(2, 2, &[0, 0, 0, 0])
    }

    fn test_entry(name: &str, raw_len: usize, payload_len: usize, format: EntryFormat) -> Entry {
        Entry {
            name: name.to_string(),
            raw_len,
            payload_len,
            offset: 0,
            format,
        }
    }

    fn raw565_fixture_with_pixels(width: u32, height: u32, pixels: &[u16]) -> Vec<u8> {
        assert_eq!(pixels.len(), width as usize * height as usize);
        let stride = width * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MM56501\0");
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&stride.to_le_bytes());
        for pixel in pixels {
            bytes.extend_from_slice(&pixel.to_le_bytes());
        }
        bytes
    }
}
