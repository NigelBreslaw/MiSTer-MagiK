use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const MAGIC: &[u8; 8] = b"MMLZ4B1\0";
const HEADER: &str = "preview_pack_bench_tsv\tlabel\tvariant\tcodec\titeration\tordinal\tasset_key\toffset\tentry_flag\tencoded_bytes\tdecoded_bytes\tcompression_ratio\twidth\theight\tload_source\tindex_lookup_us\tread_us\tdecode_us\traw565_parse_us\ttotal_us\tdecode_mb_s\ttotal_mb_s\tchecksum32\tresult\terror";

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
    total_us: u64,
    decode_mb_s: f64,
    total_mb_s: f64,
    checksum32: u32,
    result: String,
    error: String,
}

pub(crate) fn run() {
    match run_inner(std::env::args().skip(2)) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("preview-pack-bench failed: {error}");
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
    println!(
        "warm_meta\tmode={}\telapsed_us={}\tloaded=1\tpack_bytes={}",
        config.warm.label(),
        warm_us,
        archive.bytes.len()
    );
    println!("{HEADER}");

    let order = ordered_indices(&archive.entries, config.order, config.sample);
    let mut scratch = Vec::new();
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
            println!("{}", row.to_tsv());
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
        pack: PathBuf::from("/media/fat/mister-magik/assets/arcade-screenshots-320x320.mmlz4b"),
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
    println!(
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
            "preview_pack_bench_tsv\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.2}\t{:.2}\t{:08x}\t{}\t{}",
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
            tsv(&self.error)
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
    if &bytes[..8] != MAGIC {
        return Err("preview archive has bad magic".to_string());
    }
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
    let decoded = decode_payload(payload, entry.raw_len, scratch);
    let decode_us = decode_t.elapsed().as_micros() as u64;
    match decoded {
        Ok((entry_flag, data)) => {
            let parse_t = Instant::now();
            match parse_raw565(data) {
                Ok((width, height, decoded_bytes, checksum32)) => {
                    let raw565_parse_us = parse_t.elapsed().as_micros() as u64;
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
                        total_us,
                        decode_mb_s: mb_per_sec(decoded_mb, decode_us),
                        total_mb_s: mb_per_sec(decoded_mb, total_us),
                        checksum32,
                        result: "ok".to_string(),
                        error: String::new(),
                    }
                }
                Err(error) => {
                    error_row(config, entry, iteration, ordinal, read_us, decode_us, error)
                }
            }
        }
        Err(error) => error_row(config, entry, iteration, ordinal, read_us, decode_us, error),
    }
}

fn decode_payload<'a>(
    payload: &'a [u8],
    raw_len: usize,
    scratch: &'a mut Vec<u8>,
) -> Result<(&'static str, &'a [u8]), String> {
    let (&flag, block) = payload
        .split_first()
        .ok_or_else(|| "empty preview archive payload".to_string())?;
    match flag {
        0 => {
            scratch.resize(raw_len, 0);
            let len = lz4_flex::block::decompress_into(block, scratch)
                .map_err(|e| format!("lz4 decode: {e}"))?;
            if len != raw_len {
                return Err(format!("lz4 decoded length got={len} expected={raw_len}"));
            }
            Ok(("lz4_block", &scratch[..len]))
        }
        1 => {
            if block.len() != raw_len {
                return Err(format!(
                    "raw stored length got={} expected={raw_len}",
                    block.len()
                ));
            }
            Ok(("raw_stored", block))
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

fn error_row(
    config: &Config,
    entry: &Entry,
    iteration: usize,
    ordinal: usize,
    read_us: u64,
    decode_us: u64,
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
                println!(
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
                println!(
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
    println!(
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
    println!(
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
        let (flag, decoded) = decode_payload(&payload, raw.len(), &mut scratch).unwrap();
        assert_eq!(flag, "raw_stored");
        assert_eq!(decoded, raw.as_slice());
        let (width, height, decoded_bytes, _) = parse_raw565(decoded).unwrap();
        assert_eq!((width, height, decoded_bytes), (2, 2, 8));
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(parse_entries(b"not-pack").is_err());
    }

    #[test]
    fn pack_size_spec_requires_system_and_path() {
        let spec = PackSizeSpec::parse("arcade=/tmp/a.mmlz4b").unwrap();
        assert_eq!(spec.system, "arcade");
        assert_eq!(spec.path, PathBuf::from("/tmp/a.mmlz4b"));
        assert!(PackSizeSpec::parse("arcade").is_err());
    }

    fn raw565_fixture() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MM56501\0");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 8]);
        bytes
    }
}
