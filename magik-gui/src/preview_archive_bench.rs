use std::fs::{self, File};
use std::hint::black_box;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

const LZ4_BLOCK_ARCHIVE_MAGIC: &[u8; 8] = b"MMLZ4B1\0";
const RAW_ARCHIVE_MAGIC: &[u8; 8] = b"MMRAWP1\0";
const RAW565_MAGIC: &[u8; 8] = b"MM56501\0";

#[derive(Clone, Debug)]
struct ArchiveEntry {
    name: String,
    raw_len: usize,
    compressed_len: usize,
    offset: u64,
}

#[derive(Clone, Debug, Default)]
struct Sample {
    read_us: u64,
    decompress_us: u64,
    decode_us: u64,
    total_us: u64,
}

#[derive(Clone, Debug, Default)]
struct TrialSummary {
    files: usize,
    bytes_read: u64,
    decoded_bytes: u64,
    read_us: u64,
    decompress_us: u64,
    decode_us: u64,
    total_us: u64,
    checksum: u64,
    samples: Vec<Sample>,
}

pub(crate) fn run() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        let bin = args
            .first()
            .map(String::as_str)
            .unwrap_or("mister-magik-fb");
        eprintln!(
            "usage: {bin} preview-archive-bench raw-dir <dir> [trials]\n       {bin} preview-archive-bench raw <archive> [trials]\n       {bin} preview-archive-bench lz4-block <archive> [trials]\n       {bin} preview-archive-bench compare-lz4-block <raw-dir> <archive> <name.rgb565>..."
        );
        std::process::exit(2);
    }
    let offset = if args.get(1).map(String::as_str) == Some("preview-archive-bench") {
        2
    } else {
        1
    };
    if args.len() <= offset + 1 {
        let bin = args
            .first()
            .map(String::as_str)
            .unwrap_or("preview-archive-bench");
        eprintln!(
            "usage: {bin} raw-dir <dir> [trials]\n       {bin} raw <archive> [trials]\n       {bin} lz4-block <archive> [trials]\n       {bin} compare-lz4-block <raw-dir> <archive> <name.rgb565>..."
        );
        std::process::exit(2);
    }
    let mode = args[offset].as_str();
    let path = Path::new(&args[offset + 1]);
    let trials = args
        .get(offset + 2)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(5)
        .max(1);

    let result = match mode {
        "raw-dir" => bench_raw_dir(path, trials),
        "raw" => bench_archive(path, trials, ArchiveCodec::Raw),
        "lz4-block" => bench_archive(path, trials, ArchiveCodec::Lz4Block),
        "compare-lz4-block" => compare_lz4_block(&args[offset + 1..]),
        _ => Err(format!("unknown preview archive bench mode: {mode}")),
    };
    if let Err(e) = result {
        eprintln!("preview_archive_bench failed: {e}");
        std::process::exit(1);
    }
}

fn compare_lz4_block(args: &[String]) -> Result<(), String> {
    if args.len() < 3 {
        return Err("compare-lz4-block needs <raw-dir> <archive> <name.rgb565>...".into());
    }
    let raw_dir = Path::new(&args[0]);
    let archive_path = Path::new(&args[1]);
    let names = &args[2..];
    let entries = read_archive_index(archive_path, ArchiveCodec::Lz4Block)?;
    let mut archive =
        File::open(archive_path).map_err(|e| format!("open {}: {e}", archive_path.display()))?;
    println!(
        "name\traw_bytes\tcompressed_bytes\traw_read_us\traw_decode_us\traw_total_us\tarchive_read_us\tarchive_decompress_us\tarchive_decode_us\tarchive_total_us\tchecksum_match"
    );
    for name in names {
        let raw_path = raw_dir.join(name);
        maybe_drop_caches();
        let raw_t = Instant::now();
        let raw_read_t = Instant::now();
        let raw = fs::read(&raw_path).map_err(|e| format!("read {}: {e}", raw_path.display()))?;
        let raw_read_us = raw_read_t.elapsed().as_micros() as u64;
        let raw_decode_t = Instant::now();
        let raw_decoded = decode_raw565_to_words(&raw)
            .map_err(|e| format!("decode {}: {e}", raw_path.display()))?;
        let raw_decode_us = raw_decode_t.elapsed().as_micros() as u64;
        let raw_total_us = raw_t.elapsed().as_micros() as u64;

        let entry = entries
            .iter()
            .find(|entry| entry.name == *name)
            .ok_or_else(|| format!("{name} missing from archive"))?;
        maybe_drop_caches();
        let archive_t = Instant::now();
        let archive_read_t = Instant::now();
        archive
            .seek(SeekFrom::Start(entry.offset))
            .map_err(|e| format!("seek {name}: {e}"))?;
        let mut compressed = vec![0u8; entry.compressed_len];
        archive
            .read_exact(&mut compressed)
            .map_err(|e| format!("read {name}: {e}"))?;
        let archive_read_us = archive_read_t.elapsed().as_micros() as u64;
        let decompress_t = Instant::now();
        let data = decode_lz4_block_entry(&compressed, entry.raw_len)
            .map_err(|e| format!("lz4 block decode {name}: {e}"))?;
        let archive_decompress_us = decompress_t.elapsed().as_micros() as u64;
        let archive_decode_t = Instant::now();
        let archive_decoded =
            decode_raw565_to_words(&data).map_err(|e| format!("decode {name}: {e}"))?;
        let archive_decode_us = archive_decode_t.elapsed().as_micros() as u64;
        let archive_total_us = archive_t.elapsed().as_micros() as u64;
        black_box(raw_decoded.words.len());
        black_box(archive_decoded.words.len());
        println!(
            "{name}\t{}\t{}\t{raw_read_us}\t{raw_decode_us}\t{raw_total_us}\t{archive_read_us}\t{archive_decompress_us}\t{archive_decode_us}\t{archive_total_us}\t{}",
            raw.len(),
            compressed.len(),
            raw_decoded.checksum == archive_decoded.checksum
        );
    }
    Ok(())
}

struct DecodedRaw565 {
    words: Vec<u16>,
    checksum: u64,
}

fn decode_raw565_to_words(data: &[u8]) -> Result<DecodedRaw565, String> {
    let (expected, mut checksum) = validate_raw565(data)?;
    let mut words = Vec::with_capacity(expected / 2);
    for chunk in data[20..].chunks_exact(2) {
        let word = u16::from_le_bytes([chunk[0], chunk[1]]);
        checksum = checksum.wrapping_mul(16_777_619) ^ u64::from(word);
        words.push(word);
    }
    Ok(DecodedRaw565 { words, checksum })
}

fn bench_raw_dir(dir: &Path, trials: usize) -> Result<(), String> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("read_dir {}: {e}", dir.display()))?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension().and_then(|s| s.to_str()) == Some("rgb565")).then_some(path)
        })
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Err(format!("no .rgb565 files in {}", dir.display()));
    }

    println!(
        "preview_archive_bench\tmode=raw-dir\tpath={}\tentries={}\ttrials={}",
        dir.display(),
        paths.len(),
        trials
    );
    print_header();
    for trial in 1..=trials {
        maybe_drop_caches();
        let mut order: Vec<usize> = (0..paths.len()).collect();
        shuffle_order(&mut order, trial as u64);
        let summary = run_raw_trial(&paths, &order)?;
        print_summary(trial, "raw-dir", 0, &summary);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum ArchiveCodec {
    Raw,
    Lz4Block,
}

impl ArchiveCodec {
    fn label(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Lz4Block => "lz4-block",
        }
    }

    fn magic(self) -> &'static [u8; 8] {
        match self {
            Self::Raw => RAW_ARCHIVE_MAGIC,
            Self::Lz4Block => LZ4_BLOCK_ARCHIVE_MAGIC,
        }
    }
}

fn bench_archive(path: &Path, trials: usize, codec: ArchiveCodec) -> Result<(), String> {
    let archive_size = fs::metadata(path)
        .map_err(|e| format!("metadata {}: {e}", path.display()))?
        .len();
    let entries = read_archive_index(path, codec)?;
    if entries.is_empty() {
        return Err(format!("archive has no entries: {}", path.display()));
    }

    println!(
        "preview_archive_bench\tmode={}\tpath={}\tarchive_bytes={}\tentries={}\ttrials={}",
        codec.label(),
        path.display(),
        archive_size,
        entries.len(),
        trials
    );
    print_header();
    for trial in 1..=trials {
        maybe_drop_caches();
        let mut order: Vec<usize> = (0..entries.len()).collect();
        shuffle_order(&mut order, trial as u64);
        let summary = run_archive_trial(path, &entries, &order, codec)?;
        print_summary(trial, codec.label(), archive_size, &summary);
    }
    Ok(())
}

fn run_raw_trial(paths: &[PathBuf], order: &[usize]) -> Result<TrialSummary, String> {
    let mut summary = TrialSummary::default();
    let trial_t = Instant::now();
    for &idx in order {
        let path = &paths[idx];
        let sample_t = Instant::now();
        let read_t = Instant::now();
        let data = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let read_us = read_t.elapsed().as_micros() as u64;
        let decode_t = Instant::now();
        let (decoded_bytes, checksum) =
            validate_raw565(&data).map_err(|e| format!("decode {}: {e}", path.display()))?;
        let decode_us = decode_t.elapsed().as_micros() as u64;
        let total_us = sample_t.elapsed().as_micros() as u64;
        black_box(checksum);
        summary.files += 1;
        summary.bytes_read += data.len() as u64;
        summary.decoded_bytes += decoded_bytes as u64;
        summary.read_us += read_us;
        summary.decode_us += decode_us;
        summary.total_us += total_us;
        summary.checksum = summary.checksum.wrapping_add(checksum);
        summary.samples.push(Sample {
            read_us,
            decompress_us: 0,
            decode_us,
            total_us,
        });
    }
    summary.total_us = trial_t.elapsed().as_micros() as u64;
    Ok(summary)
}

fn run_archive_trial(
    path: &Path,
    entries: &[ArchiveEntry],
    order: &[usize],
    codec: ArchiveCodec,
) -> Result<TrialSummary, String> {
    let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut summary = TrialSummary::default();
    let trial_t = Instant::now();
    for &idx in order {
        let entry = &entries[idx];
        let sample_t = Instant::now();
        let read_t = Instant::now();
        file.seek(SeekFrom::Start(entry.offset))
            .map_err(|e| format!("seek {}: {e}", entry.name))?;
        let mut compressed = vec![0u8; entry.compressed_len];
        file.read_exact(&mut compressed)
            .map_err(|e| format!("read {}: {e}", entry.name))?;
        let read_us = read_t.elapsed().as_micros() as u64;
        let bytes_read = compressed.len() as u64;
        let decompress_t = Instant::now();
        let data = match codec {
            ArchiveCodec::Raw => compressed,
            ArchiveCodec::Lz4Block => decode_lz4_block_entry(&compressed, entry.raw_len)
                .map_err(|e| format!("lz4 block decode {}: {e}", entry.name))?,
        };
        let decompress_us = decompress_t.elapsed().as_micros() as u64;
        if data.len() != entry.raw_len {
            return Err(format!(
                "{} raw length mismatch got={} expected={}",
                entry.name,
                data.len(),
                entry.raw_len
            ));
        }
        let decode_t = Instant::now();
        let (decoded_bytes, checksum) =
            validate_raw565(&data).map_err(|e| format!("decode {}: {e}", entry.name))?;
        let decode_us = decode_t.elapsed().as_micros() as u64;
        let total_us = sample_t.elapsed().as_micros() as u64;
        black_box(checksum);
        summary.files += 1;
        summary.bytes_read += bytes_read;
        summary.decoded_bytes += decoded_bytes as u64;
        summary.read_us += read_us;
        summary.decompress_us += decompress_us;
        summary.decode_us += decode_us;
        summary.total_us += total_us;
        summary.checksum = summary.checksum.wrapping_add(checksum);
        summary.samples.push(Sample {
            read_us,
            decompress_us,
            decode_us,
            total_us,
        });
    }
    summary.total_us = trial_t.elapsed().as_micros() as u64;
    Ok(summary)
}

fn decode_lz4_block_entry(data: &[u8], raw_len: usize) -> Result<Vec<u8>, String> {
    let (&flag, block) = data
        .split_first()
        .ok_or_else(|| "empty lz4 block entry".to_string())?;
    match flag {
        0 => lz4_flex::block::decompress(block, raw_len).map_err(|e| e.to_string()),
        1 => {
            if block.len() != raw_len {
                return Err(format!(
                    "raw lz4 block length mismatch got={} expected={raw_len}",
                    block.len()
                ));
            }
            Ok(block.to_vec())
        }
        other => Err(format!("bad lz4 block flag {other}")),
    }
}

fn read_archive_index(path: &Path, codec: ArchiveCodec) -> Result<Vec<ArchiveEntry>, String> {
    let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)
        .map_err(|e| format!("read magic: {e}"))?;
    if &magic != codec.magic() {
        return Err("bad archive magic".into());
    }
    let count = read_u32(&mut file)? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = read_u16(&mut file)? as usize;
        let raw_len = read_u32(&mut file)? as usize;
        let compressed_len = read_u32(&mut file)? as usize;
        let offset = read_u64(&mut file)?;
        let mut name_bytes = vec![0u8; name_len];
        file.read_exact(&mut name_bytes)
            .map_err(|e| format!("read entry name: {e}"))?;
        let name = String::from_utf8(name_bytes).map_err(|e| format!("entry name utf8: {e}"))?;
        entries.push(ArchiveEntry {
            name,
            raw_len,
            compressed_len,
            offset,
        });
    }
    Ok(entries)
}

fn validate_raw565(data: &[u8]) -> Result<(usize, u64), String> {
    if data.len() < 20 || &data[..8] != RAW565_MAGIC {
        return Err("bad raw565 header".into());
    }
    let width = u32::from_le_bytes(data[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(data[12..16].try_into().unwrap());
    let stride_bytes = u32::from_le_bytes(data[16..20].try_into().unwrap());
    let min_stride = width as usize * 2;
    if stride_bytes as usize % 16 != 0 || (stride_bytes as usize) < min_stride {
        return Err(format!("bad stride width={width} stride={stride_bytes}"));
    }
    let expected = stride_bytes as usize * height as usize;
    if data.len() - 20 != expected {
        return Err(format!(
            "length mismatch got={} expected={}",
            data.len() - 20,
            expected
        ));
    }
    let mut checksum = width as u64 ^ ((height as u64) << 16) ^ ((stride_bytes as u64) << 32);
    for chunk in data[20..].chunks_exact(257).take(16) {
        checksum = checksum.wrapping_mul(16_777_619) ^ u64::from(chunk[0]);
    }
    Ok((expected, checksum))
}

fn maybe_drop_caches() {
    if matches!(
        std::env::var("MISTER_BENCH_DROP_CACHES").as_deref(),
        Ok("1") | Ok("on") | Ok("true") | Ok("yes")
    ) {
        let _ = fs::write("/proc/sys/vm/drop_caches", b"3\n");
    }
}

fn shuffle_order(order: &mut [usize], seed: u64) {
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    for i in (1..order.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let j = (state as usize) % (i + 1);
        order.swap(i, j);
    }
}

fn print_header() {
    println!(
        "trial\tmode\tarchive_bytes\tfiles\tbytes_read\tdecoded_bytes\ttotal_us\tavg_us\tp50_us\tp95_us\tmax_us\tread_us\tdecompress_us\tdecode_us\tchecksum"
    );
}

fn print_summary(trial: usize, mode: &str, archive_bytes: u64, summary: &TrialSummary) {
    let mut totals: Vec<u64> = summary.samples.iter().map(|s| s.total_us).collect();
    totals.sort_unstable();
    let avg = if summary.files == 0 {
        0
    } else {
        summary.total_us / summary.files as u64
    };
    let p50 = percentile(&totals, 0.50);
    let p95 = percentile(&totals, 0.95);
    let max = totals.last().copied().unwrap_or(0);
    black_box(summary.samples.iter().fold(0u64, |acc, s| {
        acc.wrapping_add(s.read_us)
            .wrapping_add(s.decompress_us)
            .wrapping_add(s.decode_us)
    }));
    println!(
        "{trial}\t{mode}\t{archive_bytes}\t{}\t{}\t{}\t{}\t{avg}\t{p50}\t{p95}\t{max}\t{}\t{}\t{}\t{}",
        summary.files,
        summary.bytes_read,
        summary.decoded_bytes,
        summary.total_us,
        summary.read_us,
        summary.decompress_us,
        summary.decode_us,
        summary.checksum
    );
}

fn percentile(sorted: &[u64], pct: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 * pct).ceil() as usize).saturating_sub(1);
    sorted[idx.min(sorted.len() - 1)]
}

fn read_u16(file: &mut File) -> Result<u16, String> {
    let mut buf = [0u8; 2];
    file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32(file: &mut File) -> Result<u32, String> {
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(file: &mut File) -> Result<u64, String> {
    let mut buf = [0u8; 8];
    file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(u64::from_le_bytes(buf))
}
