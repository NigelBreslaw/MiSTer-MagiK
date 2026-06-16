use image::{imageops::FilterType, RgbImage};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use rayon::prelude::*;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use ssh2::{ExtendedData, Session};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_FB_W: usize = 1920;
const DEFAULT_FB_H: usize = 1080;
const DEFAULT_FB_BPP: usize = 32;
const RAW_REBOOT_REMOTE_CMD: &str = "nohup /sbin/reboot >/dev/null 2>&1 & echo raw";
const SUPERVISED_REBOOT_REMOTE_CMD: &str = "if [ -p /dev/MiSTer_cmd ] && pidof MiSTer_MagiK >/dev/null 2>&1; then printf 'mister_magik_reboot\\n' > /dev/MiSTer_cmd; echo supervised; else echo 'supervised reboot unavailable: MiSTer_MagiK or /dev/MiSTer_cmd missing; use --raw only for recovery' >&2; exit 12; fi";

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() {
    if let Err(e) = run_cli() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run_cli() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        usage();
        return Ok(());
    }
    let action = args.remove(0);
    match action.as_str() {
        "run" => {
            let stream = args.first().map(|s| s.as_str()) == Some("--stream");
            if stream {
                args.remove(0);
            }
            let command = args.first().ok_or("run needs a command")?;
            validate_remote_run_command(command)?;
            let sess = connect(10)?;
            if stream {
                stream_command(&sess, command)?;
            } else {
                let out = exec(&sess, command, true)?;
                print!("{}", out.stdout);
                if !out.stderr.trim().is_empty() {
                    eprint!("[stderr] {}", out.stderr);
                }
                std::process::exit(out.rc);
            }
        }
        "put" => {
            if args.len() < 2 {
                return Err("put needs <local> <remote>".into());
            }
            let sess = connect(10)?;
            put(&sess, Path::new(&args[0]), &args[1])?;
            println!("put {} -> {}", args[0], args[1]);
        }
        "get" => {
            if args.len() < 2 {
                return Err("get needs <remote> <local>".into());
            }
            let sess = connect(10)?;
            get(&sess, &args[0], Path::new(&args[1]))?;
            println!("get {} -> {}", args[0], args[1]);
        }
        "db" | "library-db" => {
            let sess = connect(10)?;
            run_library_db_query(&sess, &args)?;
        }
        "wait" => {
            let secs = args.first().and_then(|s| s.parse().ok()).unwrap_or(120.0);
            std::process::exit(wait_up(secs)?);
        }
        "reboot" | "reboot-wait" => {
            let raw = take_raw_reboot_flag(&mut args);
            let host = host();
            {
                let sess = connect(10)?;
                let issued = issue_reboot(&sess, raw)?;
                println!("reboot issued to {host} ({issued})");
            }
            if action == "reboot-wait" {
                wait_down(40.0);
                let secs = args.first().and_then(|s| s.parse().ok()).unwrap_or(120.0);
                std::process::exit(wait_up(secs)?);
            }
        }
        "status" => {
            let json_out = args.iter().any(|a| a == "--json");
            let sess = connect(10)?;
            let status = collect_status(&sess)?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                print_status_summary(&status);
            }
        }
        "doctor" => {
            let json_out = args.iter().any(|a| a == "--json");
            let sess = connect(10)?;
            let status = collect_status(&sess)?;
            let findings = doctor_findings(&status);
            if json_out {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({"status": status, "findings": findings}))?
                );
            } else {
                print_status_summary(&status);
                println!("\nDoctor findings");
                for (level, text) in findings {
                    println!("  [{level}] {text}");
                }
            }
        }
        "snapshot" => {
            let sess = connect(10)?;
            let out_dir = args.first().map(PathBuf::from);
            snapshot(&sess, out_dir)?;
        }
        "boot-capture" => {
            let keep_enabled = args.iter().any(|a| a == "--keep-enabled");
            let deploy = args.iter().any(|a| a == "--deploy");
            let settle = option_value(&args, "--settle")
                .and_then(|s| s.parse().ok())
                .unwrap_or(10);
            boot_capture(deploy, keep_enabled, settle)?;
        }
        "display-read" => {
            let unsafe_spi = args.iter().any(|a| a == "--unsafe-spi");
            let json_out = args.iter().any(|a| a == "--json");
            let sess = connect(10)?;
            display_read(&sess, unsafe_spi, json_out)?;
        }
        "ini-repair-boot" => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let sess = connect(10)?;
            edit_remote_ini(&sess, IniEdit::MagikBoot, dry_run)?;
        }
        "ini-repair-arcade-video" => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let sess = connect(10)?;
            edit_remote_ini(&sess, IniEdit::ArcadeVideo, dry_run)?;
        }
        "ini-restore-stock" => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let sess = connect(10)?;
            edit_remote_ini(&sess, IniEdit::StockBoot, dry_run)?;
        }
        "ini-zaparoo-boot" => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let sess = connect(10)?;
            edit_remote_ini(&sess, IniEdit::ZaparooBoot, dry_run)?;
        }
        "ini-edit-local" => {
            validate_ini_edit_local_args(&args)?;
            let edit = parse_ini_edit_args(&args)?;
            let input = args
                .get(args.len() - 2)
                .ok_or("ini-edit-local needs <input> <output>")?;
            let output = args.last().ok_or("ini-edit-local needs <input> <output>")?;
            let text = fs::read_to_string(input)?;
            let edited = edit_mister_ini(&text, edit);
            fs::write(output, edited)?;
        }
        "profile-summary" => {
            let path = args
                .first()
                .ok_or("profile-summary needs <frame-profile.tsv>")?;
            profile_summary(Path::new(path))?;
        }
        "raw-to-png" => {
            if args.len() < 4 {
                return Err("raw-to-png needs <raw> <width> <height> <out.png>".into());
            }
            let width = args[1].parse::<usize>()?;
            let height = args[2].parse::<usize>()?;
            raw_to_png(Path::new(&args[0]), width, height, Path::new(&args[3]))?;
        }
        "preview-cache-build" => {
            preview_cache_build(&args)?;
        }
        "mame-metadata-build" => {
            mame_metadata_build(&args)?;
        }
        "recover" => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            if !dry_run {
                return Err("recover currently supports --dry-run only".into());
            }
            let sess = connect(10)?;
            let status = collect_status(&sess)?;
            println!("Dry-run recovery suggestions");
            for (_, text) in doctor_findings(&status) {
                println!("  - {text}");
            }
            println!("  - Mutating recovery is intentionally not implemented yet.");
        }
        "-h" | "--help" => usage(),
        other => return Err(format!("unknown action: {other}").into()),
    }
    Ok(())
}

fn usage() {
    println!(
        "usage: scripts/mister <run|put|get|db|library-db|wait|reboot|reboot-wait|status|doctor|snapshot|boot-capture|display-read|ini-repair-boot|ini-repair-arcade-video|ini-restore-stock|ini-zaparoo-boot|ini-edit-local|profile-summary|raw-to-png|preview-cache-build|mame-metadata-build|recover> ...\n       reboot/reboot-wait use mister_magik_reboot when available; pass --raw for recovery"
    );
}

fn take_raw_reboot_flag(args: &mut Vec<String>) -> bool {
    if let Some(pos) = args.iter().position(|arg| arg == "--raw") {
        args.remove(pos);
        true
    } else {
        false
    }
}

fn reboot_remote_command(raw: bool) -> &'static str {
    if raw {
        RAW_REBOOT_REMOTE_CMD
    } else {
        SUPERVISED_REBOOT_REMOTE_CMD
    }
}

fn issue_reboot(sess: &Session, raw: bool) -> Result<String> {
    let out = exec(sess, reboot_remote_command(raw), true)?;
    let mode = out.stdout.trim();
    if mode.is_empty() {
        Ok(if raw { "raw" } else { "unknown" }.to_string())
    } else {
        Ok(mode.to_string())
    }
}

fn validate_remote_run_command(command: &str) -> Result<()> {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let direct_arcade = [
        "mister-magik-fb ui arcade",
        "mister-magik-fb' ui arcade",
        "mister-magik-fb\" ui arcade",
        "mister-magic-fb ui arcade",
        "mister-magic-fb' ui arcade",
        "mister-magic-fb\" ui arcade",
    ]
    .iter()
    .any(|needle| normalized.contains(needle));
    if direct_arcade {
        return Err("refusing removed direct arcade scene; benchmark Arcade through the Main-supervised launcher env/restart path".into());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewResizeChoice {
    Nearest,
    Lanczos,
    Unchanged,
}

impl PreviewResizeChoice {
    fn label(self) -> &'static str {
        match self {
            Self::Nearest => "nearest",
            Self::Lanczos => "lanczos",
            Self::Unchanged => "unchanged",
        }
    }
}

#[derive(Clone, Debug)]
struct PreviewCacheJob {
    input: PathBuf,
    stem: String,
}

#[derive(Clone, Debug)]
struct PreviewCacheResult {
    file: String,
    source_w: u32,
    source_h: u32,
    output_w: u32,
    output_h: u32,
    resize: PreviewResizeChoice,
    raw_bytes: u64,
}

#[derive(Clone, Debug)]
struct PreviewArchiveSummary {
    path: PathBuf,
    entries: usize,
    raw_bytes: u64,
    payload_bytes: u64,
    archive_bytes: u64,
}

#[derive(Clone, Debug)]
struct PreviewArchiveEntry {
    name_bytes: Vec<u8>,
    raw_len: u32,
    payload_len: u32,
    offset: u64,
}

fn preview_cache_build(args: &[String]) -> Result<()> {
    let input = option_value(args, "--input")
        .or_else(|| option_value(args, "-i"))
        .ok_or("preview-cache-build needs --input <dir>")?;
    let output = option_value(args, "--output")
        .or_else(|| option_value(args, "-o"))
        .ok_or("preview-cache-build needs --output <dir>")?;
    let max_size = option_value(args, "--max")
        .as_deref()
        .unwrap_or("320")
        .parse::<u32>()?;
    if max_size == 0 {
        return Err("--max must be greater than zero".into());
    }

    let input = PathBuf::from(input);
    let output = PathBuf::from(output);
    let png_dir = output.join(format!("png-hybrid-{max_size}x{max_size}"));
    let raw_dir = output.join(format!("raw565-hybrid-{max_size}x{max_size}"));
    let archive_path = output.join(format!(
        "raw565-hybrid-{max_size}x{max_size}-lz4block-12.mmlz4b"
    ));
    fs::create_dir_all(&png_dir)?;
    fs::create_dir_all(&raw_dir)?;

    let jobs = preview_cache_jobs(&input)?;
    let total_t = Instant::now();
    let results: Vec<_> = jobs
        .par_iter()
        .map(|job| build_preview_cache_one(job, &png_dir, &raw_dir, max_size))
        .collect();

    let mut ok = Vec::with_capacity(results.len());
    let mut errors = Vec::new();
    for result in results {
        match result {
            Ok(result) => ok.push(result),
            Err(err) => errors.push(err),
        }
    }
    if !errors.is_empty() {
        for err in &errors {
            eprintln!("preview-cache-build: {err}");
        }
        return Err(format!("{} preview cache conversion(s) failed", errors.len()).into());
    }

    ok.sort_by(|a, b| a.file.cmp(&b.file));
    let nearest = ok
        .iter()
        .filter(|r| r.resize == PreviewResizeChoice::Nearest)
        .count();
    let lanczos = ok
        .iter()
        .filter(|r| r.resize == PreviewResizeChoice::Lanczos)
        .count();
    let unchanged = ok
        .iter()
        .filter(|r| r.resize == PreviewResizeChoice::Unchanged)
        .count();
    let raw_bytes: u64 = ok.iter().map(|r| r.raw_bytes).sum();
    let archive = build_preview_archive(&raw_dir, &archive_path)?;

    println!(
        "preview_cache_build input={} output={} max={} threads={}",
        input.display(),
        output.display(),
        max_size,
        rayon::current_num_threads()
    );
    println!("file\tsource_w\tsource_h\toutput_w\toutput_h\tfilter\traw_bytes");
    for result in &ok {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            result.file,
            result.source_w,
            result.source_h,
            result.output_w,
            result.output_h,
            result.resize.label(),
            result.raw_bytes
        );
    }
    println!(
        "preview_cache_summary ok={} failed=0 elapsed_ms={} nearest={} lanczos={} unchanged={} raw_bytes={} png_dir={} raw565_dir={} archive={}",
        ok.len(),
        total_t.elapsed().as_millis(),
        nearest,
        lanczos,
        unchanged,
        raw_bytes,
        png_dir.display(),
        raw_dir.display(),
        archive.path.display()
    );
    println!(
        "preview_archive codec=lz4-block entries={} raw_bytes={} compressed_payload_bytes={} archive_bytes={} preset=rust-block output={}",
        archive.entries,
        archive.raw_bytes,
        archive.payload_bytes,
        archive.archive_bytes,
        archive.path.display()
    );
    Ok(())
}

fn build_preview_archive(raw_dir: &Path, out_path: &Path) -> Result<PreviewArchiveSummary> {
    let mut files = Vec::new();
    for entry in fs::read_dir(raw_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("rgb565") {
            continue;
        }
        files.push(path);
    }
    files.sort();
    if files.is_empty() {
        return Err(format!("no .rgb565 files in {}", raw_dir.display()).into());
    }

    let mut entries = Vec::with_capacity(files.len());
    let mut payloads = Vec::with_capacity(files.len());
    for path in files {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("non-utf8 raw565 filename: {}", path.display()))?
            .to_string();
        let name_bytes = name.as_bytes().to_vec();
        if name_bytes.len() > u16::MAX as usize {
            return Err(format!("preview archive name too long: {name}").into());
        }
        let raw = fs::read(&path)?;
        if raw.len() > u32::MAX as usize {
            return Err(format!("preview archive file too large: {}", path.display()).into());
        }
        let compressed = lz4_flex::block::compress(&raw);
        let payload = if compressed.len() < raw.len() {
            let mut payload = Vec::with_capacity(1 + compressed.len());
            payload.push(0);
            payload.extend_from_slice(&compressed);
            payload
        } else {
            let mut payload = Vec::with_capacity(1 + raw.len());
            payload.push(1);
            payload.extend_from_slice(&raw);
            payload
        };
        if payload.len() > u32::MAX as usize {
            return Err(format!("preview archive payload too large: {}", path.display()).into());
        }
        entries.push(PreviewArchiveEntry {
            name_bytes,
            raw_len: raw.len() as u32,
            payload_len: payload.len() as u32,
            offset: 0,
        });
        payloads.push(payload);
    }

    let mut index_len = 8usize + 4;
    for entry in &entries {
        index_len = index_len
            .checked_add(2 + 4 + 4 + 8 + entry.name_bytes.len())
            .ok_or("preview archive index length overflow")?;
    }
    let mut offset = index_len as u64;
    for entry in &mut entries {
        entry.offset = offset;
        offset = offset
            .checked_add(entry.payload_len as u64)
            .ok_or("preview archive payload offset overflow")?;
    }

    let mut bytes = Vec::with_capacity(offset as usize);
    bytes.extend_from_slice(b"MMLZ4B1\0");
    bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in &entries {
        bytes.extend_from_slice(&(entry.name_bytes.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&entry.raw_len.to_le_bytes());
        bytes.extend_from_slice(&entry.payload_len.to_le_bytes());
        bytes.extend_from_slice(&entry.offset.to_le_bytes());
        bytes.extend_from_slice(&entry.name_bytes);
    }
    for payload in &payloads {
        bytes.extend_from_slice(payload);
    }
    fs::write(out_path, &bytes)?;

    Ok(PreviewArchiveSummary {
        path: out_path.to_path_buf(),
        entries: entries.len(),
        raw_bytes: entries.iter().map(|entry| entry.raw_len as u64).sum(),
        payload_bytes: entries.iter().map(|entry| entry.payload_len as u64).sum(),
        archive_bytes: bytes.len() as u64,
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
struct MameMachine {
    setname: String,
    parent_setname: Option<String>,
    title: String,
    year: Option<String>,
    manufacturer: Option<String>,
    sourcefile: Option<String>,
    rotate: Option<i64>,
    display_type: Option<String>,
    display_width: Option<i64>,
    display_height: Option<i64>,
    refresh_hz: Option<f64>,
    players: Option<i64>,
    coins: Option<i64>,
    control_type: Option<String>,
    control_ways: Option<String>,
    buttons: Option<i64>,
    driver_status: Option<String>,
    emulation_status: Option<String>,
    savestate: Option<String>,
    source_version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MameSoftwareItem {
    list_name: String,
    software_name: String,
    parent_name: Option<String>,
    description: String,
    year: Option<String>,
    publisher: Option<String>,
    region: Option<String>,
    source_version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MameSoftwareHash {
    list_name: String,
    software_name: String,
    part_name: Option<String>,
    rom_name: Option<String>,
    size: Option<i64>,
    crc32: Option<String>,
    sha1: Option<String>,
    data_area: Option<String>,
    disk_sha1: Option<String>,
}

fn mame_metadata_build(args: &[String]) -> Result<()> {
    let out = option_value(args, "--out")
        .or_else(|| option_value(args, "-o"))
        .ok_or("mame-metadata-build needs --out <sqlite>")?;
    let machines = if let Some(machine_sqlite) = option_value(args, "--machine-sqlite") {
        load_mame_machines_from_db(Path::new(&machine_sqlite))?
    } else {
        let xml = if let Some(listxml) = option_value(args, "--listxml") {
            fs::read_to_string(listxml)?
        } else {
            let mame = option_value(args, "--mame")
            .or_else(|| env::var("MAME_BIN").ok())
            .or_else(|| find_program_on_path("mame"))
            .ok_or("mame-metadata-build needs --listxml <xml>, --mame <binary>, MAME_BIN, or mame on PATH")?;
            let output = Command::new(&mame).arg("-listxml").output()?;
            if !output.status.success() {
                return Err(format!(
                    "{mame} -listxml failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )
                .into());
            }
            String::from_utf8(output.stdout)?
        };
        parse_mame_listxml(&xml)?
    };
    let (software_items, software_hashes) = load_mame_software_lists(args)?;
    write_mame_metadata_db(
        Path::new(&out),
        &machines,
        &software_items,
        &software_hashes,
    )?;
    println!(
        "mame_metadata_build out={} machines={} software_items={} software_hashes={} source_version={}",
        out,
        machines.len(),
        software_items.len(),
        software_hashes.len(),
        machines
            .first()
            .map(|machine| machine.source_version.as_str())
            .unwrap_or("unknown")
    );
    Ok(())
}

fn load_mame_software_lists(
    args: &[String],
) -> Result<(Vec<MameSoftwareItem>, Vec<MameSoftwareHash>)> {
    const TARGET_LISTS: &[&str] = &["nes", "snes", "n64", "sms", "megadriv", "saturn"];

    let mut paths = option_values(args, "--software-list")
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if let Some(dir) = option_value(args, "--software-dir") {
        let dir = PathBuf::from(dir);
        for list in TARGET_LISTS {
            let path = dir.join(format!("{list}.xml"));
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths.dedup();

    let mut items = Vec::new();
    let mut hashes = Vec::new();
    for path in paths {
        let xml = fs::read_to_string(&path)?;
        let (mut list_items, mut list_hashes) = parse_mame_software_list_xml(&xml)?;
        items.append(&mut list_items);
        hashes.append(&mut list_hashes);
    }
    Ok((items, hashes))
}

fn parse_mame_listxml(xml: &str) -> Result<Vec<MameMachine>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut machines = Vec::new();
    let mut source_version = "unknown".to_string();
    let mut current: Option<MameMachine> = None;
    let mut field = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                match tag.as_str() {
                    "mame" => {
                        if let Some(build) = attr_value(&e, b"build") {
                            source_version = build;
                        }
                    }
                    "machine" => {
                        let setname = attr_value(&e, b"name").unwrap_or_default();
                        current = Some(MameMachine {
                            setname,
                            parent_setname: attr_value(&e, b"cloneof"),
                            sourcefile: attr_value(&e, b"sourcefile"),
                            source_version: source_version.clone(),
                            ..MameMachine::default()
                        });
                    }
                    "description" | "year" | "manufacturer" if current.is_some() => field = tag,
                    "input" => {
                        if let Some(machine) = current.as_mut() {
                            apply_mame_input(machine, &e);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if let Some(machine) = current.as_mut() {
                    match tag.as_str() {
                        "display" if machine.display_type.is_none() => {
                            apply_mame_display(machine, &e)
                        }
                        "input" => apply_mame_input(machine, &e),
                        "control" => apply_mame_control(machine, &e),
                        "driver" => apply_mame_driver(machine, &e),
                        _ => {}
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(machine) = current.as_mut() {
                    let text = e.xml10_content().unwrap_or_default().into_owned();
                    match field.as_str() {
                        "description" => machine.title = text,
                        "year" => machine.year = Some(text),
                        "manufacturer" => machine.manufacturer = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if tag == "machine" {
                    if let Some(mut machine) = current.take() {
                        if machine.title.is_empty() {
                            machine.title = machine.setname.clone();
                        }
                        machines.push(machine);
                    }
                }
                field.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("parse MAME listxml: {e}").into()),
            _ => {}
        }
    }
    Ok(machines)
}

fn parse_mame_software_list_xml(
    xml: &str,
) -> Result<(Vec<MameSoftwareItem>, Vec<MameSoftwareHash>)> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut items = Vec::new();
    let mut hashes = Vec::new();
    let mut list_name = String::new();
    let mut source_version = "software-list".to_string();
    let mut current: Option<MameSoftwareItem> = None;
    let mut current_part: Option<String> = None;
    let mut current_data_area: Option<String> = None;
    let mut field = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                match tag.as_str() {
                    "softwarelist" => {
                        list_name = attr_value(&e, b"name").unwrap_or_default();
                        if let Some(build) = attr_value(&e, b"build") {
                            source_version = build;
                        }
                    }
                    "software" => {
                        let software_name = attr_value(&e, b"name").unwrap_or_default();
                        current = Some(MameSoftwareItem {
                            list_name: list_name.clone(),
                            software_name,
                            parent_name: attr_value(&e, b"cloneof"),
                            source_version: source_version.clone(),
                            ..MameSoftwareItem::default()
                        });
                    }
                    "description" | "year" | "publisher" if current.is_some() => field = tag,
                    "part" if current.is_some() => current_part = attr_value(&e, b"name"),
                    "dataarea" | "diskarea" if current.is_some() => {
                        current_data_area = attr_value(&e, b"name")
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if let Some(item) = current.as_ref() {
                    match tag.as_str() {
                        "rom" => hashes.push(MameSoftwareHash {
                            list_name: item.list_name.clone(),
                            software_name: item.software_name.clone(),
                            part_name: current_part.clone(),
                            rom_name: attr_value(&e, b"name"),
                            size: attr_value(&e, b"size").and_then(|value| value.parse().ok()),
                            crc32: attr_value(&e, b"crc").map(|value| value.to_ascii_lowercase()),
                            sha1: attr_value(&e, b"sha1").map(|value| value.to_ascii_lowercase()),
                            data_area: current_data_area.clone(),
                            disk_sha1: None,
                        }),
                        "disk" => hashes.push(MameSoftwareHash {
                            list_name: item.list_name.clone(),
                            software_name: item.software_name.clone(),
                            part_name: current_part.clone(),
                            rom_name: attr_value(&e, b"name"),
                            size: None,
                            crc32: None,
                            sha1: None,
                            data_area: current_data_area.clone(),
                            disk_sha1: attr_value(&e, b"sha1")
                                .map(|value| value.to_ascii_lowercase()),
                        }),
                        _ => {}
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(item) = current.as_mut() {
                    let text = e.xml10_content().unwrap_or_default().into_owned();
                    match field.as_str() {
                        "description" => {
                            item.description = text;
                            item.region = region_from_text(&item.description).map(str::to_string);
                        }
                        "year" => item.year = Some(text),
                        "publisher" => item.publisher = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                match tag.as_str() {
                    "software" => {
                        if let Some(mut item) = current.take() {
                            if item.description.is_empty() {
                                item.description = item.software_name.clone();
                            }
                            if item.region.is_none() {
                                item.region =
                                    region_from_text(&item.description).map(str::to_string);
                            }
                            items.push(item);
                        }
                        current_part = None;
                        current_data_area = None;
                    }
                    "part" => current_part = None,
                    "dataarea" | "diskarea" => current_data_area = None,
                    "description" | "year" | "publisher" => field.clear(),
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("parse software list XML: {e}").into()),
            _ => {}
        }
    }

    Ok((items, hashes))
}

fn load_mame_machines_from_db(path: &Path) -> Result<Vec<MameMachine>> {
    let conn = Connection::open(path)?;
    let mut stmt = conn.prepare(
        "SELECT setname,parent_setname,title,year,manufacturer,sourcefile,rotate,display_type,
                display_width,display_height,refresh_hz,players,coins,control_type,control_ways,
                buttons,driver_status,emulation_status,savestate,source_version
         FROM mame_machines
         ORDER BY setname",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(MameMachine {
            setname: row.get(0)?,
            parent_setname: row.get(1)?,
            title: row.get(2)?,
            year: row.get(3)?,
            manufacturer: row.get(4)?,
            sourcefile: row.get(5)?,
            rotate: row.get(6)?,
            display_type: row.get(7)?,
            display_width: row.get(8)?,
            display_height: row.get(9)?,
            refresh_hz: row.get(10)?,
            players: row.get(11)?,
            coins: row.get(12)?,
            control_type: row.get(13)?,
            control_ways: row.get(14)?,
            buttons: row.get(15)?,
            driver_status: row.get(16)?,
            emulation_status: row.get(17)?,
            savestate: row.get(18)?,
            source_version: row.get(19)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn region_from_text(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    if contains_any(
        &lower,
        &["(usa", "(us)", "(u)", "[usa", "[us]", " usa", " ntsc-u"],
    ) {
        Some("usa")
    } else if contains_any(
        &lower,
        &[
            "(europe", "(eu", "(e)", "[europe", "[eu]", " europe", " pal",
        ],
    ) {
        Some("europe")
    } else if contains_any(
        &lower,
        &[
            "(japan", "(jp", "(j)", "[japan", "[jp]", " japan", " ntsc-j",
        ],
    ) {
        Some("japan")
    } else if contains_any(&lower, &["(korea", "[korea", " korea"]) {
        Some("korea")
    } else if contains_any(&lower, &["(world", "(w)", "[world", " world"]) {
        Some("world")
    } else {
        None
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn find_program_on_path(name: &str) -> Option<String> {
    let paths = env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        env::var_os("PATHEXT")
            .map(|value| {
                value
                    .to_string_lossy()
                    .split(';')
                    .filter(|ext| !ext.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_else(|| vec![".exe".into(), ".bat".into(), ".cmd".into()])
    } else {
        vec![String::new()]
    };
    for dir in env::split_paths(&paths) {
        for extension in &extensions {
            let candidate = if extension.is_empty() || name.ends_with(extension) {
                dir.join(name)
            } else {
                dir.join(format!("{name}{extension}"))
            };
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        }
    }
    None
}

fn apply_mame_display(machine: &mut MameMachine, e: &BytesStart<'_>) {
    machine.display_type = attr_value(e, b"type");
    machine.rotate = attr_value(e, b"rotate").and_then(|value| value.parse().ok());
    machine.display_width = attr_value(e, b"width").and_then(|value| value.parse().ok());
    machine.display_height = attr_value(e, b"height").and_then(|value| value.parse().ok());
    machine.refresh_hz = attr_value(e, b"refresh").and_then(|value| value.parse().ok());
}

fn apply_mame_input(machine: &mut MameMachine, e: &BytesStart<'_>) {
    machine.players = attr_value(e, b"players").and_then(|value| value.parse().ok());
    machine.coins = attr_value(e, b"coins").and_then(|value| value.parse().ok());
}

fn apply_mame_control(machine: &mut MameMachine, e: &BytesStart<'_>) {
    if machine.control_type.is_none() {
        machine.control_type = attr_value(e, b"type");
    }
    if machine.control_ways.is_none() {
        machine.control_ways = attr_value(e, b"ways");
    }
    if let Some(buttons) = attr_value(e, b"buttons").and_then(|value| value.parse::<i64>().ok()) {
        machine.buttons = Some(machine.buttons.unwrap_or(0).max(buttons));
    }
}

fn apply_mame_driver(machine: &mut MameMachine, e: &BytesStart<'_>) {
    machine.driver_status = attr_value(e, b"status");
    machine.emulation_status = attr_value(e, b"emulation");
    machine.savestate = attr_value(e, b"savestate");
}

fn attr_value(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|attr| attr.key.as_ref() == key)
        .map(|attr| String::from_utf8_lossy(attr.value.as_ref()).into_owned())
}

fn write_mame_metadata_db(
    path: &Path,
    machines: &[MameMachine],
    software_items: &[MameSoftwareItem],
    software_hashes: &[MameSoftwareHash],
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("sqlite3.tmp");
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut conn = Connection::open(&tmp)?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=OFF;
        PRAGMA synchronous=OFF;
        CREATE TABLE mame_machines (
            setname TEXT PRIMARY KEY,
            parent_setname TEXT,
            title TEXT NOT NULL,
            year TEXT,
            manufacturer TEXT,
            sourcefile TEXT,
            rotate INTEGER,
            display_type TEXT,
            display_width INTEGER,
            display_height INTEGER,
            refresh_hz REAL,
            players INTEGER,
            coins INTEGER,
            control_type TEXT,
            control_ways TEXT,
            buttons INTEGER,
            driver_status TEXT,
            emulation_status TEXT,
            savestate TEXT,
            source_version TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE mame_software_items (
            list_name TEXT NOT NULL,
            software_name TEXT NOT NULL,
            parent_name TEXT,
            description TEXT NOT NULL,
            year TEXT,
            publisher TEXT,
            region TEXT,
            source_version TEXT NOT NULL,
            PRIMARY KEY(list_name, software_name)
        ) WITHOUT ROWID;
        CREATE TABLE mame_software_hashes (
            list_name TEXT NOT NULL,
            software_name TEXT NOT NULL,
            part_name TEXT,
            rom_name TEXT,
            size INTEGER,
            crc32 TEXT,
            sha1 TEXT,
            data_area TEXT,
            disk_sha1 TEXT
        );
        CREATE INDEX mame_software_hashes_crc_idx
            ON mame_software_hashes(list_name, size, crc32);
        CREATE INDEX mame_software_hashes_disk_idx
            ON mame_software_hashes(list_name, disk_sha1);
        "#,
    )?;
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO mame_machines(
                setname,parent_setname,title,year,manufacturer,sourcefile,rotate,display_type,
                display_width,display_height,refresh_hz,players,coins,control_type,control_ways,
                buttons,driver_status,emulation_status,savestate,source_version
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
        )?;
        for machine in machines {
            stmt.execute(params![
                machine.setname,
                machine.parent_setname,
                machine.title,
                machine.year,
                machine.manufacturer,
                machine.sourcefile,
                machine.rotate,
                machine.display_type,
                machine.display_width,
                machine.display_height,
                machine.refresh_hz,
                machine.players,
                machine.coins,
                machine.control_type,
                machine.control_ways,
                machine.buttons,
                machine.driver_status,
                machine.emulation_status,
                machine.savestate,
                machine.source_version
            ])?;
        }
    }
    {
        let mut stmt = tx.prepare(
            "INSERT INTO mame_software_items(
                list_name,software_name,parent_name,description,year,publisher,region,source_version
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        )?;
        for item in software_items {
            stmt.execute(params![
                item.list_name,
                item.software_name,
                item.parent_name,
                item.description,
                item.year,
                item.publisher,
                item.region,
                item.source_version
            ])?;
        }
    }
    {
        let mut stmt = tx.prepare(
            "INSERT INTO mame_software_hashes(
                list_name,software_name,part_name,rom_name,size,crc32,sha1,data_area,disk_sha1
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        )?;
        for hash in software_hashes {
            stmt.execute(params![
                hash.list_name,
                hash.software_name,
                hash.part_name,
                hash.rom_name,
                hash.size,
                hash.crc32,
                hash.sha1,
                hash.data_area,
                hash.disk_sha1
            ])?;
        }
    }
    tx.commit()?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn preview_cache_jobs(input: &Path) -> Result<Vec<PreviewCacheJob>> {
    let mut jobs = Vec::new();
    let mut stems = std::collections::HashSet::new();
    for entry in fs::read_dir(input)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with("._") {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("non-utf8 source stem: {}", path.display()))?
            .to_string();
        if !stems.insert(stem.clone()) {
            return Err(format!("duplicate source stem: {stem}").into());
        }
        jobs.push(PreviewCacheJob { input: path, stem });
    }
    jobs.sort_by(|a, b| a.input.cmp(&b.input));
    Ok(jobs)
}

fn build_preview_cache_one(
    job: &PreviewCacheJob,
    png_dir: &Path,
    raw_dir: &Path,
    max_size: u32,
) -> std::result::Result<PreviewCacheResult, String> {
    let image = image::open(&job.input)
        .map_err(|e| format!("decode {}: {e}", job.input.display()))?
        .to_rgb8();
    let source_w = image.width();
    let source_h = image.height();
    let (resized, resize) = resize_preview_image(image, max_size);

    let png_path = png_dir.join(format!("{}.png", job.stem));
    resized
        .save(&png_path)
        .map_err(|e| format!("write {}: {e}", png_path.display()))?;

    let raw_path = raw_dir.join(format!("{}.rgb565", job.stem));
    let raw = encode_raw565_preview(&resized);
    fs::write(&raw_path, &raw).map_err(|e| format!("write {}: {e}", raw_path.display()))?;

    Ok(PreviewCacheResult {
        file: job
            .input
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&job.stem)
            .to_string(),
        source_w,
        source_h,
        output_w: resized.width(),
        output_h: resized.height(),
        resize,
        raw_bytes: raw.len() as u64,
    })
}

fn resize_preview_image(image: RgbImage, max_size: u32) -> (RgbImage, PreviewResizeChoice) {
    let Some((target_w, target_h, scale)) =
        preview_target_size(image.width(), image.height(), max_size)
    else {
        return (image, PreviewResizeChoice::Unchanged);
    };
    if scale > 1.0 {
        (
            image::imageops::resize(&image, target_w, target_h, FilterType::Nearest),
            PreviewResizeChoice::Nearest,
        )
    } else {
        (
            image::imageops::resize(&image, target_w, target_h, FilterType::Lanczos3),
            PreviewResizeChoice::Lanczos,
        )
    }
}

fn preview_target_size(width: u32, height: u32, max_size: u32) -> Option<(u32, u32, f64)> {
    let scale = (max_size as f64 / width as f64).min(max_size as f64 / height as f64);
    let target_w = ((width as f64 * scale).round() as u32).max(1);
    let target_h = ((height as f64 * scale).round() as u32).max(1);
    if target_w == width && target_h == height {
        None
    } else {
        Some((target_w, target_h, scale))
    }
}

fn encode_raw565_preview(image: &RgbImage) -> Vec<u8> {
    let stride_bytes = align16(image.width() as usize * 2);
    let payload_len = stride_bytes * image.height() as usize;
    let mut bytes = Vec::with_capacity(20 + payload_len);
    bytes.extend_from_slice(b"MM56501\0");
    bytes.extend_from_slice(&image.width().to_le_bytes());
    bytes.extend_from_slice(&image.height().to_le_bytes());
    bytes.extend_from_slice(&(stride_bytes as u32).to_le_bytes());
    bytes.resize(20 + payload_len, 0);
    for y in 0..image.height() as usize {
        let dst_row = 20 + y * stride_bytes;
        for x in 0..image.width() as usize {
            let pixel = image.get_pixel(x as u32, y as u32).0;
            let word = rgb8_to_rgb565_word(pixel[0], pixel[1], pixel[2]);
            let dst = dst_row + x * 2;
            bytes[dst..dst + 2].copy_from_slice(&word.to_le_bytes());
        }
    }
    bytes
}

fn rgb8_to_rgb565_word(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 & 0xf8) << 8) | ((g as u16 & 0xfc) << 3) | (b as u16 >> 3)
}

fn align16(n: usize) -> usize {
    (n + 15) & !15
}

fn host() -> String {
    env::var("MISTER_IP").unwrap_or_else(|_| "192.168.1.117".to_string())
}

fn user() -> String {
    env::var("MISTER_USER").unwrap_or_else(|_| "root".to_string())
}

fn pass() -> String {
    env::var("MISTER_PASS").unwrap_or_else(|_| "1".to_string())
}

fn connect(timeout_secs: u64) -> Result<Session> {
    let addr = format!("{}:22", host())
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer host")?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(timeout_secs))?;
    tcp.set_read_timeout(Some(Duration::from_secs(timeout_secs)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(timeout_secs)))?;
    let mut sess = Session::new()?;
    sess.set_tcp_stream(tcp);
    sess.handshake()?;
    sess.userauth_password(&user(), &pass())?;
    if !sess.authenticated() {
        return Err("SSH password authentication failed".into());
    }
    Ok(sess)
}

struct ExecOutput {
    rc: i32,
    stdout: String,
    stderr: String,
}

fn exec(sess: &Session, command: &str, merge_stderr: bool) -> Result<ExecOutput> {
    let mut channel = sess.channel_session()?;
    if merge_stderr {
        channel.handle_extended_data(ExtendedData::Merge)?;
    }
    channel.exec(command)?;
    let mut stdout = String::new();
    channel.read_to_string(&mut stdout)?;
    let mut stderr = String::new();
    if !merge_stderr {
        channel.stderr().read_to_string(&mut stderr)?;
    }
    channel.wait_close()?;
    Ok(ExecOutput {
        rc: channel.exit_status()?,
        stdout,
        stderr,
    })
}

fn stream_command(sess: &Session, command: &str) -> Result<()> {
    let mut channel = sess.channel_session()?;
    channel.handle_extended_data(ExtendedData::Merge)?;
    channel.exec(command)?;
    let mut buf = [0u8; 8192];
    loop {
        match channel.read(&mut buf) {
            Ok(0) => {
                if channel.eof() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Ok(n) => {
                io::stdout().write_all(&buf[..n])?;
                io::stdout().flush()?;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
    channel.wait_close()?;
    std::process::exit(channel.exit_status()?);
}

fn put(sess: &Session, local: &Path, remote: &str) -> Result<()> {
    let sftp = sess.sftp()?;
    let mut src = File::open(local)?;
    let mut dst = sftp.create(Path::new(remote))?;
    io::copy(&mut src, &mut dst)?;
    Ok(())
}

fn get(sess: &Session, remote: &str, local: &Path) -> Result<()> {
    if let Some(parent) = local.parent() {
        fs::create_dir_all(parent)?;
    }
    let sftp = sess.sftp()?;
    let mut src = sftp.open(Path::new(remote))?;
    let mut dst = File::create(local)?;
    io::copy(&mut src, &mut dst)?;
    Ok(())
}

fn run_library_db_query(sess: &Session, args: &[String]) -> Result<()> {
    let query_args = if args.is_empty() {
        vec![
            "SELECT".to_string(),
            "type,name,tbl_name".to_string(),
            "FROM".to_string(),
            "sqlite_schema".to_string(),
            "WHERE".to_string(),
            "type".to_string(),
            "IN".to_string(),
            "('table','view')".to_string(),
            "ORDER".to_string(),
            "BY".to_string(),
            "type,name".to_string(),
        ]
    } else {
        args.to_vec()
    };
    let quoted_args = query_args
        .iter()
        .map(|arg| sh(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let command = format!("/media/fat/mister-magik/mister-magik-fb library-sql {quoted_args}");
    let out = exec(sess, &command, true)?;
    print!("{}", out.stdout);
    if !out.stderr.trim().is_empty() {
        eprint!("[stderr] {}", out.stderr);
    }
    std::process::exit(out.rc);
}

fn remote_write(sess: &Session, remote: &str, bytes: &[u8]) -> Result<()> {
    let sftp = sess.sftp()?;
    let mut dst = sftp.create(Path::new(remote))?;
    dst.write_all(bytes)?;
    Ok(())
}

fn port_open(timeout: Duration) -> bool {
    let Ok(mut addrs) = format!("{}:22", host()).to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, timeout).is_ok()
}

fn userspace_ready() -> Option<String> {
    let sess = connect(4).ok()?;
    let out = exec(&sess, "pidof MiSTer || echo BOOTING", true).ok()?;
    Some(out.stdout.trim().to_string())
}

fn wait_down(max_seconds: f64) -> bool {
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < max_seconds {
        if !port_open(Duration::from_secs(2)) {
            println!(
                "  device went down after {:.1}s",
                start.elapsed().as_secs_f64()
            );
            return true;
        }
        thread::sleep(Duration::from_secs(1));
    }
    println!("  (device still answering; proceeding to wait-up anyway)");
    false
}

fn wait_up(max_seconds: f64) -> Result<i32> {
    let start = Instant::now();
    let mut attempt = 0;
    while start.elapsed().as_secs_f64() < max_seconds {
        attempt += 1;
        let elapsed = start.elapsed().as_secs_f64();
        if port_open(Duration::from_millis(1500)) {
            if let Some(status) = userspace_ready() {
                let mister = if status == "BOOTING" {
                    "booting".to_string()
                } else {
                    format!("pid {status}")
                };
                println!(
                    "SSH ready after {:.1}s (attempt {attempt}); MiSTer {mister}",
                    start.elapsed().as_secs_f64()
                );
                return Ok(0);
            }
        }
        println!("  [{elapsed:5.1}s] waiting for ssh...");
        thread::sleep(Duration::from_secs(1));
    }
    println!("TIMEOUT: device not ready after {max_seconds:.0}s");
    Ok(1)
}

fn remote_read(sess: &Session, path: &str) -> Option<String> {
    let cmd = format!("cat {} 2>/dev/null", sh(path));
    let out = exec(sess, &cmd, true).ok()?;
    if out.rc == 0 {
        Some(out.stdout)
    } else {
        None
    }
}

fn remote_trim(sess: &Session, path: &str) -> Option<String> {
    remote_read(sess, path).map(|s| s.trim().to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum IniEdit {
    MagikBoot,
    ZaparooBoot,
    ArcadeVideo,
    MenuMode(String),
    MenuAuto,
    Crt {
        direct_video: String,
        menu_pal: String,
        forced_scandoubler: String,
    },
    CommentMain,
    StockBoot,
}

fn parse_ini_edit_args(args: &[String]) -> Result<IniEdit> {
    match args.first().map(String::as_str) {
        Some("magik-boot") => Ok(IniEdit::MagikBoot),
        Some("zaparoo-boot") => Ok(IniEdit::ZaparooBoot),
        Some("arcade-video") => Ok(IniEdit::ArcadeVideo),
        Some("menu-mode") => {
            let mode = args.get(1).ok_or("menu-mode needs <mode>")?;
            Ok(IniEdit::MenuMode(mode.clone()))
        }
        Some("menu-auto") => Ok(IniEdit::MenuAuto),
        Some("crt") => {
            if args.len() < 4 {
                return Err("crt needs <direct_video> <menu_pal> <forced_scandoubler>".into());
            }
            Ok(IniEdit::Crt {
                direct_video: args[1].clone(),
                menu_pal: args[2].clone(),
                forced_scandoubler: args[3].clone(),
            })
        }
        Some("comment-main") => Ok(IniEdit::CommentMain),
        Some("stock-boot") => Ok(IniEdit::StockBoot),
        Some(other) => Err(format!("unknown ini edit: {other}").into()),
        None => Err("ini edit mode is required".into()),
    }
}

fn validate_ini_edit_local_args(args: &[String]) -> Result<()> {
    let expected = match args.first().map(String::as_str) {
        Some(
            "magik-boot" | "zaparoo-boot" | "arcade-video" | "menu-auto" | "comment-main"
            | "stock-boot",
        ) => 3,
        Some("menu-mode") => 4,
        Some("crt") => 6,
        Some(other) => return Err(format!("unknown ini edit: {other}").into()),
        None => return Err("ini edit mode is required".into()),
    };
    if args.len() != expected {
        return Err(
            "ini-edit-local needs <magik-boot|zaparoo-boot|arcade-video|menu-mode|menu-auto|crt|comment-main|stock-boot> ... <input> <output>"
                .into(),
        );
    }
    Ok(())
}

fn edit_remote_ini(sess: &Session, edit: IniEdit, dry_run: bool) -> Result<()> {
    const INI: &str = "/media/fat/MiSTer.ini";
    let input = remote_read(sess, INI).ok_or("could not read /media/fat/MiSTer.ini")?;
    let edited = edit_mister_ini(&input, edit);
    if dry_run {
        print!("{edited}");
        return Ok(());
    }
    let tmp = "/media/fat/MiSTer.ini.mister-tool-new";
    remote_write(sess, tmp, edited.as_bytes())?;
    let out = exec(sess, &format!("mv {} {} && sync", sh(tmp), sh(INI)), true)?;
    if out.rc != 0 {
        return Err(format!("failed to replace {INI}: {}", out.stdout).into());
    }
    println!("MiSTer.ini edited with comment-preserving Rust mutator");
    Ok(())
}

fn edit_mister_ini(input: &str, edit: IniEdit) -> String {
    let newline = if input.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = input
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();

    match edit {
        IniEdit::MagikBoot => {
            set_ini_key(&mut lines, "MiSTer", "direct_video", "0");
            set_ini_key(&mut lines, "MiSTer", "main", "MiSTer_MagiK");
            set_ini_key(&mut lines, "Menu", "direct_video", "0");
            set_ini_key(&mut lines, "Menu", "video_mode", "8");
        }
        IniEdit::ZaparooBoot => {
            set_ini_key(&mut lines, "MiSTer", "direct_video", "0");
            set_ini_key(&mut lines, "MiSTer", "main", "zaparoo/MiSTer_Zaparoo");
            set_ini_key(&mut lines, "Menu", "direct_video", "0");
            set_ini_key(&mut lines, "Menu", "video_mode", "8");
        }
        IniEdit::ArcadeVideo => {
            set_ini_key(&mut lines, "MiSTer", "direct_video", "0");
            set_ini_key(&mut lines, "arcade", "direct_video", "1");
            set_ini_key(&mut lines, "arcade_vertical", "direct_video", "0");
            set_ini_key(&mut lines, "arcade_vertical", "video_mode", "8");
            set_ini_key(&mut lines, "arcade_vertical", "vscale_mode", "1");
            ensure_section_after(&mut lines, "arcade", "arcade_vertical");
        }
        IniEdit::MenuMode(mode) => {
            set_ini_key(&mut lines, "Menu", "video_mode", &mode);
        }
        IniEdit::MenuAuto => {
            comment_ini_key(
                &mut lines,
                Some("Menu"),
                "video_mode",
                "MiSTer MagiK EDID/native video-mode probe",
            );
        }
        IniEdit::Crt {
            direct_video,
            menu_pal,
            forced_scandoubler,
        } => {
            set_ini_key(
                &mut lines,
                "MiSTer",
                "forced_scandoubler",
                &forced_scandoubler,
            );
            set_ini_key(&mut lines, "MiSTer", "menu_pal", &menu_pal);
            set_ini_key(&mut lines, "MiSTer", "direct_video", &direct_video);
        }
        IniEdit::CommentMain => {
            comment_ini_key(
                &mut lines,
                Some("MiSTer"),
                "main",
                "MiSTer MagiK disabled for stock probe",
            );
        }
        IniEdit::StockBoot => {
            comment_ini_key_if_value(
                &mut lines,
                Some("MiSTer"),
                "main",
                &["MiSTer_MagiK", "mister-magik-fb"],
                "MiSTer MagiK stock boot restore",
            );
        }
    }

    let mut out = lines.join(newline);
    if input.ends_with('\n') {
        out.push_str(newline);
    }
    out
}

fn set_ini_key(lines: &mut Vec<String>, section: &str, key: &str, value: &str) {
    let mut current = String::from("global");
    let mut saw_section = false;
    let mut changed = false;
    let mut insert_at = None;

    for (idx, line) in lines.iter_mut().enumerate() {
        if let Some(name) = section_name(line) {
            if current.eq_ignore_ascii_case(section) && insert_at.is_none() {
                insert_at = Some(idx);
            }
            current = name;
            if current.eq_ignore_ascii_case(section) {
                saw_section = true;
            }
            continue;
        }

        if current.eq_ignore_ascii_case(section) && active_key_eq(line, key) {
            *line = replace_assignment_value(line, value);
            changed = true;
        }
    }

    if changed {
        return;
    }

    if current.eq_ignore_ascii_case(section) && insert_at.is_none() {
        insert_at = Some(lines.len());
    }

    if saw_section {
        lines.insert(insert_at.unwrap_or(lines.len()), format!("{key}={value}"));
    } else {
        if !lines.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
            lines.push(String::new());
        }
        lines.push(format!("[{section}]"));
        lines.push(format!("{key}={value}"));
    }
}

fn ensure_section_after(lines: &mut Vec<String>, earlier: &str, later: &str) {
    let Some(earlier_range) = section_range(lines, earlier) else {
        return;
    };
    let Some(later_range) = section_range(lines, later) else {
        return;
    };
    if earlier_range.start < later_range.start {
        return;
    }

    let later_len = later_range.end - later_range.start;
    let moved: Vec<String> = lines.drain(later_range.clone()).collect();
    let adjusted_earlier_end = earlier_range.end.saturating_sub(later_len);
    lines.splice(adjusted_earlier_end..adjusted_earlier_end, moved);
}

fn section_range(lines: &[String], section: &str) -> Option<std::ops::Range<usize>> {
    let start = lines.iter().position(|line| {
        section_name(line).is_some_and(|name| name.eq_ignore_ascii_case(section))
    })?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| section_name(line).is_some())
        .map(|offset| start + 1 + offset)
        .unwrap_or(lines.len());
    Some(start..end)
}

fn comment_ini_key(lines: &mut [String], section: Option<&str>, key: &str, reason: &str) {
    let mut current = String::from("global");
    for line in lines {
        if let Some(name) = section_name(line) {
            current = name;
            continue;
        }
        let section_matches = section
            .map(|name| current.eq_ignore_ascii_case(name))
            .unwrap_or(true);
        if section_matches && active_key_eq(line, key) {
            *line = format!(";{} ; {}", line, reason);
        }
    }
}

fn comment_ini_key_if_value(
    lines: &mut [String],
    section: Option<&str>,
    key: &str,
    values: &[&str],
    reason: &str,
) {
    let mut current = String::from("global");
    for line in lines {
        if let Some(name) = section_name(line) {
            current = name;
            continue;
        }
        let section_matches = section
            .map(|name| current.eq_ignore_ascii_case(name))
            .unwrap_or(true);
        if section_matches
            && active_key_eq(line, key)
            && assignment_value(line).is_some_and(|value| {
                values
                    .iter()
                    .any(|expected| value.eq_ignore_ascii_case(expected))
            })
        {
            *line = format!(";{} ; {}", line, reason);
        }
    }
}

fn section_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with(';') || trimmed.starts_with('#') || !trimmed.starts_with('[') {
        return None;
    }
    let end = trimmed.find(']')?;
    Some(trimmed[1..end].trim().to_string())
}

fn active_key_eq(line: &str, expected: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('#') {
        return false;
    }
    let Some((key, _)) = trimmed.split_once('=') else {
        return false;
    };
    key.trim().eq_ignore_ascii_case(expected)
}

fn replace_assignment_value(line: &str, value: &str) -> String {
    let Some(eq) = line.find('=') else {
        return line.to_string();
    };
    let after_eq = &line[eq + 1..];
    let value_start = after_eq
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(idx, _)| idx)
        .unwrap_or(after_eq.len());
    let after_value_start = &after_eq[value_start..];
    let comment_pos = after_value_start
        .char_indices()
        .find(|(_, ch)| *ch == ';' || *ch == '#')
        .map(|(idx, _)| idx);
    let suffix = comment_pos
        .map(|pos| {
            let before_comment = &after_value_start[..pos];
            let whitespace_start = before_comment
                .char_indices()
                .rev()
                .find(|(_, ch)| !ch.is_whitespace())
                .map(|(idx, ch)| idx + ch.len_utf8())
                .unwrap_or(0);
            &after_value_start[whitespace_start..]
        })
        .unwrap_or("");
    format!("{}{}{}", &line[..eq + 1 + value_start], value, suffix)
}

fn assignment_value(line: &str) -> Option<String> {
    let (_, after_eq) = line.split_once('=')?;
    let value = after_eq
        .split([';', '#'])
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    Some(value)
}

fn collect_status(sess: &Session) -> Result<Value> {
    let visual = fb_visual_sample(sess)?;
    let main_status = parse_json(remote_read(sess, "/tmp/mister-magik/main-status.json"));
    let slint_status = parse_json(remote_read(sess, "/tmp/mister-magik/status.json"));
    let owner = main_status
        .as_ref()
        .and_then(|v| v.get("visible_owner"))
        .and_then(Value::as_str);
    let visual_class = visual.get("class").and_then(Value::as_str);
    let fb0_visible_candidate = owner == Some("fb0")
        && !matches!(visual_class, None | Some("mostly_black") | Some("unknown"));
    Ok(json!({
        "schema": "mister-magik-status-v1",
        "collected_at_unix": unix_secs(),
        "device": {
            "hostname": remote_trim(sess, "/proc/sys/kernel/hostname"),
            "uptime": remote_trim(sess, "/proc/uptime"),
            "arch": exec_stdout(sess, "uname -m")?.trim(),
        },
        "processes": {
            "MiSTer": process_list(sess, "MiSTer")?,
            "MiSTer_MagiK": process_list(sess, "MiSTer_MagiK")?,
            "mister-magik-fb": process_list(sess, "mister-magik-fb")?,
        },
        "boot": {
            "ini_keys": parse_ini_keys(remote_read(sess, "/media/fat/MiSTer.ini").unwrap_or_default()),
            "inittab": lines_containing(remote_read(sess, "/etc/inittab").unwrap_or_default(), &["MiSTer", "mister-magik"]),
        },
        "display": {
            "proc_fb": remote_trim(sess, "/proc/fb"),
            "fb_mode": remote_trim(sess, "/sys/module/MiSTer_fb/parameters/mode"),
            "virtual_size": remote_trim(sess, "/sys/class/graphics/fb0/virtual_size"),
            "bits_per_pixel": remote_trim(sess, "/sys/class/graphics/fb0/bits_per_pixel"),
            "stride": remote_trim(sess, "/sys/class/graphics/fb0/stride"),
            "active_vt": remote_trim(sess, "/sys/class/tty/tty0/active"),
            "fb0_visual": visual,
            "fb0_visible_candidate": fb0_visible_candidate,
        },
        "runtime": {
            "slint_status": slint_status,
            "main_status": main_status,
            "events_tail": tail_remote(sess, "/tmp/mister-magik/events.jsonl", 30),
            "logs": {
                "main": tail_remote(sess, "/tmp/mister-magik-main.log", 20),
                "slint": tail_remote(sess, "/tmp/mister-magik-slint.log", 20),
            }
        },
        "input": {
            "devices": parse_input_devices(remote_read(sess, "/proc/bus/input/devices").unwrap_or_default()),
        },
        "owners": fd_owners(sess)?,
        "audio": {
            "mr_audio_exists": exec(sess, "[ -e /dev/MrAudio ]", true)?.rc == 0,
        }
    }))
}

fn parse_json(text: Option<String>) -> Option<Value> {
    text.and_then(|s| serde_json::from_str(&s).ok())
}

fn exec_stdout(sess: &Session, cmd: &str) -> Result<String> {
    Ok(exec(sess, cmd, true)?.stdout)
}

fn process_list(sess: &Session, name: &str) -> Result<Vec<Value>> {
    let pids = exec_stdout(sess, &format!("pidof {} 2>/dev/null || true", sh(name)))?;
    let mut out = Vec::new();
    for pid in pids
        .split_whitespace()
        .filter_map(|s| s.parse::<u32>().ok())
    {
        let status = remote_read(sess, &format!("/proc/{pid}/status")).unwrap_or_default();
        let mut item = serde_json::Map::new();
        item.insert("pid".to_string(), json!(pid));
        for line in status.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            if matches!(
                k,
                "Name" | "State" | "PPid" | "VmRSS" | "Threads" | "Cpus_allowed_list"
            ) {
                item.insert(k.to_ascii_lowercase(), json!(v.trim()));
            }
        }
        item.insert("pid".to_string(), json!(pid));
        let cmd = exec_stdout(
            sess,
            &format!("tr '\\0' ' ' < /proc/{pid}/cmdline 2>/dev/null || true"),
        )?;
        item.insert("cmdline".to_string(), json!(cmd.trim()));
        out.push(Value::Object(item));
    }
    Ok(out)
}

fn parse_ini_keys(text: String) -> Value {
    let mut root = serde_json::Map::new();
    let mut section = "global".to_string();
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.contains(']') {
            section = line[1..line.find(']').unwrap()].to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if matches!(
            key,
            "main" | "video_mode" | "direct_video" | "fb_terminal" | "fb_size"
        ) {
            let sec = root.entry(section.clone()).or_insert_with(|| json!({}));
            sec.as_object_mut().unwrap().insert(
                key.to_string(),
                json!({"value": value.trim(), "line": idx + 1}),
            );
        }
    }
    Value::Object(root)
}

fn lines_containing(text: String, needles: &[&str]) -> Vec<String> {
    text.lines()
        .filter(|line| needles.iter().any(|n| line.contains(n)))
        .map(ToString::to_string)
        .collect()
}

fn tail_remote(sess: &Session, path: &str, n: usize) -> Option<Vec<String>> {
    let out = exec(sess, &format!("tail -n {n} {} 2>/dev/null", sh(path)), true).ok()?;
    if out.rc == 0 {
        Some(out.stdout.lines().map(ToString::to_string).collect())
    } else {
        None
    }
}

fn parse_input_devices(text: String) -> Vec<Value> {
    let mut out = Vec::new();
    let mut current = serde_json::Map::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                out.push(Value::Object(std::mem::take(&mut current)));
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("N: Name=") {
            current.insert("name".to_string(), json!(rest.trim().trim_matches('"')));
        } else if let Some(rest) = line.strip_prefix("H: Handlers=") {
            current.insert(
                "handlers".to_string(),
                json!(rest.split_whitespace().collect::<Vec<_>>()),
            );
        } else if let Some(rest) = line.strip_prefix("I: ") {
            current.insert("id".to_string(), json!(rest.trim()));
        }
    }
    if !current.is_empty() {
        out.push(Value::Object(current));
    }
    out
}

fn fd_owners(sess: &Session) -> Result<Value> {
    let script = r#"
for name in MiSTer MiSTer_MagiK mister-magik-fb; do
  for p in $(pidof "$name" 2>/dev/null); do
    for fd in /proc/$p/fd/*; do
      t=$(readlink "$fd" 2>/dev/null || true)
      case "$t" in
        /dev/fb0|/dev/mem|/dev/tty0|/dev/tty2|/dev/MiSTer_cmd|/dev/MrAudio|/dev/uinput|/dev/input/*)
          echo "$p	$name	${fd##*/}	$t"
          ;;
      esac
    done
  done
done
"#;
    let rows = exec_stdout(sess, script)?;
    let mut by_device = serde_json::Map::new();
    let mut by_process = serde_json::Map::new();
    for line in rows.lines() {
        let parts: Vec<_> = line.split('\t').collect();
        if parts.len() != 4 {
            continue;
        }
        let pid = parts[0].parse::<u32>().unwrap_or(0);
        let fd = parts[2].parse::<u32>().unwrap_or(0);
        let proc_item = json!({"pid": pid, "process": parts[1], "fd": fd, "target": parts[3]});
        by_device
            .entry(parts[3].to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(json!({"pid": pid, "process": parts[1], "fd": fd}));
        by_process
            .entry(parts[0].to_string())
            .or_insert_with(|| json!({"process": parts[1], "fds": []}))
            .get_mut("fds")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(proc_item);
    }
    Ok(json!({"by_device": by_device, "by_process": by_process}))
}

fn fb_visual_sample(sess: &Session) -> Result<Value> {
    let capture = capture_fb(sess, "status")?;
    Ok(classify_fb(&capture.raw, &capture.geometry))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FbGeometry {
    width: usize,
    height: usize,
    stride: usize,
    bpp: usize,
}

impl FbGeometry {
    fn bytes(self) -> Result<usize> {
        self.stride
            .checked_mul(self.height)
            .ok_or_else(|| "framebuffer byte size overflow".into())
    }
}

struct FbCapture {
    raw: Vec<u8>,
    geometry: FbGeometry,
}

fn framebuffer_geometry(sess: &Session) -> Result<FbGeometry> {
    let virtual_size = remote_trim(sess, "/sys/class/graphics/fb0/virtual_size")
        .unwrap_or_else(|| format!("{DEFAULT_FB_W},{DEFAULT_FB_H}"));
    let (width, height) = parse_virtual_size(&virtual_size).unwrap_or((DEFAULT_FB_W, DEFAULT_FB_H));
    let bpp = remote_trim(sess, "/sys/class/graphics/fb0/bits_per_pixel")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_FB_BPP);
    let bytes_per_pixel = bpp
        .checked_div(8)
        .filter(|v| *v > 0)
        .ok_or_else(|| format!("unsupported framebuffer bpp: {bpp}"))?;
    let packed_stride = width
        .checked_mul(bytes_per_pixel)
        .ok_or("framebuffer stride overflow")?;
    let stride = remote_trim(sess, "/sys/class/graphics/fb0/stride")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(packed_stride);
    if stride < packed_stride {
        return Err(format!(
            "framebuffer stride {stride} is smaller than packed row {packed_stride}"
        )
        .into());
    }
    Ok(FbGeometry {
        width,
        height,
        stride,
        bpp,
    })
}

fn parse_virtual_size(text: &str) -> Option<(usize, usize)> {
    let (w, h) = text.trim().split_once(',')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn capture_fb(sess: &Session, label: &str) -> Result<FbCapture> {
    let geometry = framebuffer_geometry(sess)?;
    let expected = geometry.bytes()?;
    let remote = format!("/tmp/mister-magik-{label}-{}.raw", unix_secs());
    let cmd = format!(
        "dd if=/dev/fb0 of={} bs={} count=1 2>/dev/null && wc -c {}",
        sh(&remote),
        expected,
        sh(&remote)
    );
    let out = exec(sess, &cmd, true)?;
    if out.rc != 0 {
        return Err(format!("failed to capture /dev/fb0: {}", out.stdout).into());
    }
    let sftp = sess.sftp()?;
    let mut file = sftp.open(Path::new(&remote))?;
    let mut raw = Vec::with_capacity(expected);
    file.read_to_end(&mut raw)?;
    let _ = sftp.unlink(Path::new(&remote));
    if raw.len() < expected {
        return Err(format!(
            "fb0 raw had {} bytes, expected {expected} for {}x{} stride={} bpp={}",
            raw.len(),
            geometry.width,
            geometry.height,
            geometry.stride,
            geometry.bpp
        )
        .into());
    }
    raw.truncate(expected);
    Ok(FbCapture { raw, geometry })
}

fn classify_fb(raw: &[u8], geometry: &FbGeometry) -> Value {
    let mut samples = 0u32;
    let mut nonzero = 0u32;
    let mut blackish = 0u32;
    let mut transitions = 0u32;
    let mut color_min = 0x00ff_ffffu32;
    let mut color_max = 0u32;
    let mut prev = None;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for y in (0..geometry.height).step_by(16) {
        for x in (0..geometry.width).step_by(16) {
            let Some((r, g, b)) = rgb_from_raw(raw, geometry, x, y) else {
                continue;
            };
            let p = (r << 16) | (g << 8) | b;
            samples += 1;
            nonzero += u32::from(p != 0);
            blackish += u32::from(r < 8 && g < 8 && b < 8);
            color_min = color_min.min(p);
            color_max = color_max.max(p);
            if let Some(prev) = prev {
                if color_distance(prev, p) > 96 {
                    transitions += 1;
                }
            }
            prev = Some(p);
            hash ^= p as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
    }
    let nonzero_pct = pct(nonzero, samples);
    let blackish_pct = pct(blackish, samples);
    let transition_pct = pct(transitions, samples.saturating_sub(1).max(1));
    let class = if blackish_pct >= 95.0 {
        "mostly_black"
    } else if nonzero_pct >= 20.0 && transition_pct >= 35.0 {
        "static_like"
    } else if nonzero_pct >= 5.0 {
        "slint_like"
    } else {
        "unknown"
    };
    json!({
        "ok": true,
        "width": geometry.width,
        "height": geometry.height,
        "stride": geometry.stride,
        "bpp": geometry.bpp,
        "step": 16,
        "samples": samples,
        "nonzero": nonzero,
        "blackish": blackish,
        "transitions": transitions,
        "nonzero_pct": round2(nonzero_pct),
        "blackish_pct": round2(blackish_pct),
        "transition_pct": round2(transition_pct),
        "color_min": format!("{color_min:06x}"),
        "color_max": format!("{color_max:06x}"),
        "class": class,
        "hash": format!("{hash:016x}"),
    })
}

fn color_distance(a: u32, b: u32) -> u32 {
    let ar = (a >> 16) & 0xff;
    let ag = (a >> 8) & 0xff;
    let ab = a & 0xff;
    let br = (b >> 16) & 0xff;
    let bg = (b >> 8) & 0xff;
    let bb = b & 0xff;
    ar.abs_diff(br) + ag.abs_diff(bg) + ab.abs_diff(bb)
}

fn pct(n: u32, d: u32) -> f64 {
    if d == 0 {
        0.0
    } else {
        n as f64 * 100.0 / d as f64
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn print_status_summary(status: &Value) {
    let display = &status["display"];
    let visual = &display["fb0_visual"];
    println!("MiSTer status");
    println!(
        "  active_vt: {}",
        display["active_vt"].as_str().unwrap_or("?")
    );
    println!(
        "  fb_mode:   {}",
        display["fb_mode"].as_str().unwrap_or("?")
    );
    println!(
        "  fb0:       {} hash={}",
        visual["class"].as_str().unwrap_or("unknown"),
        visual["hash"].as_str().unwrap_or("?")
    );
    println!(
        "  boot:      [MiSTer] main={} direct_video={} [Menu] direct_video={} video_mode={}",
        ini_value(status, "MiSTer", "main").unwrap_or("?"),
        ini_value(status, "MiSTer", "direct_video").unwrap_or("?"),
        ini_value(status, "Menu", "direct_video").unwrap_or("?"),
        ini_value(status, "Menu", "video_mode").unwrap_or("?")
    );
    println!(
        "  arcade:   [arcade] direct_video={} [arcade_vertical] direct_video={} video_mode={}",
        ini_value(status, "arcade", "direct_video").unwrap_or("?"),
        ini_value(status, "arcade_vertical", "direct_video").unwrap_or("?"),
        ini_value(status, "arcade_vertical", "video_mode").unwrap_or("?")
    );
    for name in ["MiSTer", "MiSTer_MagiK", "mister-magik-fb"] {
        let pid = primary_process(status, name)
            .and_then(|v| v["pid"].as_u64())
            .map(|p| p.to_string())
            .unwrap_or_else(|| "none".to_string());
        println!("  {name:<15} pid={pid}");
    }
    if let Some(main) = status["runtime"]["main_status"].as_object() {
        println!(
            "  main:      visible_owner={} launcher_pid={} osd_suppressed={}",
            main.get("visible_owner")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            main.get("launcher_pid")
                .map(Value::to_string)
                .unwrap_or_else(|| "?".into()),
            main.get("osd_suppressed_count")
                .map(Value::to_string)
                .unwrap_or_else(|| "?".into())
        );
    }
    if let Some(slint) = status["runtime"]["slint_status"].as_object() {
        println!(
            "  slint:     scene={} screen={} fps={} frames={}",
            slint.get("scene").and_then(Value::as_str).unwrap_or("?"),
            slint.get("screen").and_then(Value::as_str).unwrap_or("?"),
            slint
                .get("fps_estimate")
                .map(Value::to_string)
                .unwrap_or_else(|| "?".into()),
            slint
                .get("frames")
                .map(Value::to_string)
                .unwrap_or_else(|| "?".into())
        );
    }
}

fn primary_process<'a>(status: &'a Value, name: &str) -> Option<&'a Value> {
    let processes = status["processes"][name].as_array()?;
    if name == "mister-magik-fb" {
        processes
            .iter()
            .find(|process| {
                process["cmdline"]
                    .as_str()
                    .is_some_and(|cmd| cmd.contains(" ui launcher "))
            })
            .or_else(|| processes.first())
    } else {
        processes.first()
    }
}

fn ini_value<'a>(status: &'a Value, section: &str, key: &str) -> Option<&'a str> {
    status["boot"]["ini_keys"][section][key]["value"].as_str()
}

fn ini_line(status: &Value, section: &str, key: &str) -> Option<u64> {
    status["boot"]["ini_keys"][section][key]["line"].as_u64()
}

fn doctor_findings(status: &Value) -> Vec<(String, String)> {
    let mut findings = Vec::new();
    if ini_value(status, "MiSTer", "main") != Some("MiSTer_MagiK") {
        findings.push(("error".into(), "[MiSTer] main is not MiSTer_MagiK".into()));
    }
    if ini_value(status, "MiSTer", "direct_video") != Some("0") {
        findings.push((
            "warn".into(),
            "[MiSTer] direct_video is not 0; launcher boot may use direct-video timings".into(),
        ));
    }
    if ini_value(status, "arcade", "direct_video") != Some("1") {
        findings.push((
            "warn".into(),
            "[arcade] direct_video is not 1; normal arcade games will use scaler output".into(),
        ));
    }
    if ini_value(status, "Menu", "direct_video") != Some("0") {
        findings.push(("error".into(), "[Menu] direct_video is not 0".into()));
    }
    if ini_value(status, "Menu", "video_mode") != Some("8") {
        findings.push(("warn".into(), "[Menu] video_mode is not 8".into()));
    }
    if ini_value(status, "arcade_vertical", "direct_video") != Some("0") {
        findings.push((
            "warn".into(),
            "[arcade_vertical] direct_video is not 0; rotated games may bypass MiSTer rotation"
                .into(),
        ));
    }
    if ini_value(status, "arcade_vertical", "video_mode") != Some("8") {
        findings.push((
            "warn".into(),
            "[arcade_vertical] video_mode is not 8; rotated games should use 1080p scaler mode"
                .into(),
        ));
    }
    if let (Some(arcade), Some(vertical)) = (
        ini_line(status, "arcade", "direct_video"),
        ini_line(status, "arcade_vertical", "direct_video"),
    ) {
        if arcade > vertical {
            findings.push((
                "warn".into(),
                "[arcade] appears after [arcade_vertical]; vertical arcade settings will be overwritten"
                    .into(),
            ));
        }
    }
    for name in ["MiSTer_MagiK", "mister-magik-fb"] {
        if status["processes"][name]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(true)
        {
            findings.push(("error".into(), format!("{name} is not running")));
        }
    }
    if status["display"]["active_vt"].as_str() != Some("tty2") {
        findings.push((
            "warn".into(),
            format!(
                "active VT is {}, expected tty2 for launcher",
                status["display"]["active_vt"].as_str().unwrap_or("?")
            ),
        ));
    }
    match status["display"]["fb0_visual"]["class"].as_str() {
        Some("mostly_black") => {
            findings.push(("error".into(), "/dev/fb0 samples as mostly_black".into()))
        }
        Some("unknown") | None => {
            findings.push(("warn".into(), "/dev/fb0 visual class is unknown".into()))
        }
        _ => {}
    }
    if let Some(owner) = status["runtime"]["main_status"]["visible_owner"].as_str() {
        if owner != "fb0" {
            findings.push((
                "warn".into(),
                format!("Main reports visible_owner={owner} rather than fb0"),
            ));
        }
    }
    let fb_owned = status["owners"]["by_device"]["/dev/fb0"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .any(|o| o["process"].as_str() == Some("mister-magik-fb"))
        })
        .unwrap_or(false);
    if !fb_owned {
        findings.push((
            "warn".into(),
            "/dev/fb0 is not owned by mister-magik-fb".into(),
        ));
    }
    if findings.is_empty() {
        findings.push((
            "ok".into(),
            "No obvious launcher/display problems found".into(),
        ));
    }
    findings
}

fn snapshot(sess: &Session, out_dir: Option<PathBuf>) -> Result<()> {
    let dir = out_dir.unwrap_or_else(|| PathBuf::from("build/device-snapshots").join(timestamp()));
    fs::create_dir_all(&dir)?;
    let capture = capture_fb(sess, "snapshot")?;
    let status = collect_status(sess)?;
    fs::write(dir.join("status.json"), serde_json::to_vec_pretty(&status)?)?;
    fs::write(dir.join("fb0.raw"), &capture.raw)?;
    write_png_bgrx_stride(&capture.raw, &capture.geometry, &dir.join("fb0.png"))?;
    println!("snapshot: {}", dir.display());
    println!("png: {}", dir.join("fb0.png").display());
    Ok(())
}

fn boot_capture(deploy: bool, keep_enabled: bool, settle_secs: u64) -> Result<()> {
    if deploy {
        return Err("boot-capture --deploy is intentionally not wired into the Rust tool yet; run deploy-main-mister-experiment.sh first".into());
    }
    {
        let sess = connect(10)?;
        let _ = exec(&sess, "mkdir -p /media/fat/mister-magik; : > /media/fat/mister-magik/boot-analytics.enabled; sync", true)?;
        let issued = issue_reboot(&sess, false)?;
        println!("reboot issued to {} ({issued})", host());
    }
    wait_down(40.0);
    if wait_up(120.0)? != 0 {
        return Err("device did not return after reboot".into());
    }
    thread::sleep(Duration::from_secs(settle_secs));
    let sess = connect(10)?;
    let dir = PathBuf::from("build/boot-analytics").join(timestamp());
    fs::create_dir_all(&dir)?;
    let status = collect_status(&sess)?;
    fs::write(dir.join("status.json"), serde_json::to_vec_pretty(&status)?)?;
    for (remote, local) in [
        ("/tmp/mister-magik-boot-analytics.tsv", "boot-analytics.tsv"),
        ("/tmp/mister-magik/events.jsonl", "events.jsonl"),
        ("/tmp/mister-magik/status.json", "slint-status.json"),
        ("/tmp/mister-magik/main-status.json", "main-status.json"),
        ("/tmp/mister-magik-slint.log", "slint.log"),
        ("/tmp/mister-magik-main.log", "main.log"),
        (
            "/tmp/mister-magik-launcher-frame-profile.tsv",
            "launcher-frame-profile.tsv",
        ),
        ("/tmp/mister-magik-visual-samples.tsv", "visual-samples.tsv"),
    ] {
        if get(&sess, remote, &dir.join(local)).is_err() {
            fs::write(dir.join(format!("{local}.missing")), remote)?;
        }
    }
    if !keep_enabled {
        let _ = exec(
            &sess,
            "rm -f /media/fat/mister-magik/boot-analytics.enabled; sync",
            true,
        );
    }
    println!("boot-capture: {}", dir.display());
    Ok(())
}

fn display_read(sess: &Session, unsafe_spi: bool, json_out: bool) -> Result<()> {
    let status = collect_status(sess)?;
    if display_read_needs_unsafe_spi(&status) && !unsafe_spi {
        return Err(
            "display-read touches FPGA SPI; pass --unsafe-spi when Main/Slint may own /dev/mem"
                .into(),
        );
    }
    let out = exec(sess, "/media/fat/mister-magik/mister-magik-fb read", true)?;
    if json_out {
        println!("{}", json!({"rc": out.rc, "output": out.stdout}));
    } else {
        print!("{}", out.stdout);
    }
    std::process::exit(out.rc);
}

fn display_read_needs_unsafe_spi(status: &Value) -> bool {
    ["MiSTer_MagiK", "MiSTer"].iter().any(|name| {
        status["processes"][name]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    })
}

fn profile_summary(path: &Path) -> Result<()> {
    print!("{}", profile_summary_text(path)?);
    Ok(())
}

fn profile_summary_text(path: &Path) -> Result<String> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    let header: Vec<_> = lines.next().ok_or("empty TSV")?.split('\t').collect();
    let rows: Vec<Vec<_>> = lines.map(|l| l.split('\t').collect()).collect();
    let mut out = String::new();
    out.push_str(&format!(
        "=== {} ({} frames) ===\n",
        path.display(),
        rows.len()
    ));
    for col in [
        "wall_us",
        "phases_us",
        "anim_us",
        "render_us",
        "vsync_us",
        "copy_us",
    ] {
        let Some(idx) = header.iter().position(|h| *h == col) else {
            continue;
        };
        let mut vals: Vec<u64> = rows
            .iter()
            .filter_map(|r| r.get(idx).and_then(|v| v.parse().ok()))
            .collect();
        if vals.is_empty() {
            continue;
        }
        vals.sort_unstable();
        let avg = vals.iter().sum::<u64>() / vals.len() as u64;
        let p50 = vals[vals.len() / 2];
        let p95 = vals[((vals.len() - 1) as f64 * 0.95) as usize];
        out.push_str(&format!(
            "{col:10} min={:6} p50={p50:6} p95={p95:6} max={:6} avg={avg:6}",
            vals[0],
            vals[vals.len() - 1]
        ));
        out.push('\n');
    }
    Ok(out)
}

fn write_png_bgrx(raw: &[u8], w: usize, h: usize, path: &Path) -> Result<()> {
    let geometry = FbGeometry {
        width: w,
        height: h,
        stride: w.checked_mul(4).ok_or("raw dimensions overflow")?,
        bpp: 32,
    };
    write_png_bgrx_stride(raw, &geometry, path)
}

fn write_png_bgrx_stride(raw: &[u8], geometry: &FbGeometry, path: &Path) -> Result<()> {
    let expected = geometry.bytes()?;
    if raw.len() < expected {
        return Err(format!(
            "raw framebuffer has {} bytes, expected at least {expected}",
            raw.len()
        )
        .into());
    }
    let w = geometry.width;
    let h = geometry.height;
    let mut rgba = Vec::with_capacity((w * 4 + 1) * h);
    for y in 0..h {
        rgba.push(0);
        for x in 0..w {
            let Some((r, g, b)) = rgb_from_raw(raw, geometry, x, y) else {
                rgba.extend_from_slice(&[0, 0, 0, 0xff]);
                continue;
            };
            rgba.push(r as u8);
            rgba.push(g as u8);
            rgba.push(b as u8);
            rgba.push(0xff);
        }
    }
    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png_chunk(&mut png, b"IHDR", &ihdr);
    png_chunk(&mut png, b"IDAT", &zlib_store(&rgba));
    png_chunk(&mut png, b"IEND", &[]);
    fs::write(path, png)?;
    Ok(())
}

fn rgb_from_raw(raw: &[u8], geometry: &FbGeometry, x: usize, y: usize) -> Option<(u32, u32, u32)> {
    match geometry.bpp {
        32 => {
            let i = y
                .checked_mul(geometry.stride)?
                .checked_add(x.checked_mul(4)?)?;
            if i + 2 >= raw.len() {
                return None;
            }
            Some((raw[i + 2] as u32, raw[i + 1] as u32, raw[i] as u32))
        }
        16 => {
            let i = y
                .checked_mul(geometry.stride)?
                .checked_add(x.checked_mul(2)?)?;
            if i + 1 >= raw.len() {
                return None;
            }
            let v = u16::from_le_bytes([raw[i], raw[i + 1]]);
            let r5 = (v >> 11) & 0x1f;
            let g6 = (v >> 5) & 0x3f;
            let b5 = v & 0x1f;
            let r = ((r5 << 3) | (r5 >> 2)) as u32;
            let g = ((g6 << 2) | (g6 >> 4)) as u32;
            let b = ((b5 << 3) | (b5 >> 2)) as u32;
            Some((r, g, b))
        }
        _ => None,
    }
}

fn raw_to_png(raw_path: &Path, w: usize, h: usize, out_path: &Path) -> Result<()> {
    let raw = fs::read(raw_path)?;
    let expected = w
        .checked_mul(h)
        .and_then(|px| px.checked_mul(4))
        .ok_or("raw dimensions overflow")?;
    if raw.len() < expected {
        return Err(format!(
            "{} has {} bytes, expected at least {expected}",
            raw_path.display(),
            raw.len()
        )
        .into());
    }
    write_png_bgrx(&raw[..expected], w, h, out_path)
}

fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut pos = 0;
    while pos < data.len() {
        let len = (data.len() - pos).min(65_535);
        let final_block = pos + len == data.len();
        out.push(if final_block { 1 } else { 0 });
        out.extend_from_slice(&(len as u16).to_le_bytes());
        out.extend_from_slice(&(!(len as u16)).to_le_bytes());
        out.extend_from_slice(&data[pos..pos + len]);
        pos += len;
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(tag.len() + data.len());
    crc_input.extend_from_slice(tag);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn sh(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

fn option_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|idx| args.get(idx + 1))
        .cloned()
}

fn option_values(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .collect()
}

fn timestamp() -> String {
    unix_secs().to_string()
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn status_fixture() -> Value {
        json!({
            "boot": {
                "ini_keys": {
                    "MiSTer": {
                        "main": {"value": "MiSTer_MagiK"},
                        "direct_video": {"value": "0"}
                    },
                    "arcade": {
                        "direct_video": {"value": "1", "line": 20}
                    },
                    "arcade_vertical": {
                        "direct_video": {"value": "0", "line": 24},
                        "video_mode": {"value": "8"}
                    },
                    "Menu": {
                        "direct_video": {"value": "0"},
                        "video_mode": {"value": "8"}
                    }
                }
            },
            "processes": {
                "MiSTer": [],
                "MiSTer_MagiK": [{"pid": 10}],
                "mister-magik-fb": [{"pid": 11}]
            },
            "display": {
                "active_vt": "tty2",
                "fb0_visual": {"class": "slint_like"}
            },
            "runtime": {
                "main_status": {"visible_owner": "fb0"}
            },
            "owners": {
                "by_device": {
                    "/dev/fb0": [{"process": "mister-magik-fb", "pid": 11, "fd": 5}]
                }
            }
        })
    }

    fn raw_frame_with<F>(f: F) -> Vec<u8>
    where
        F: FnMut(usize, usize) -> (u8, u8, u8),
    {
        raw_frame_with_geometry(default_fb_geometry(), f)
    }

    fn default_fb_geometry() -> FbGeometry {
        FbGeometry {
            width: DEFAULT_FB_W,
            height: DEFAULT_FB_H,
            stride: DEFAULT_FB_W * DEFAULT_FB_BPP / 8,
            bpp: DEFAULT_FB_BPP,
        }
    }

    fn raw_frame_with_geometry<F>(geometry: FbGeometry, mut f: F) -> Vec<u8>
    where
        F: FnMut(usize, usize) -> (u8, u8, u8),
    {
        let mut raw = vec![0; geometry.bytes().unwrap()];
        for y in 0..geometry.height {
            for x in 0..geometry.width {
                let (r, g, b) = f(x, y);
                let i = y * geometry.stride + x * 4;
                raw[i] = b;
                raw[i + 1] = g;
                raw[i + 2] = r;
                raw[i + 3] = 0xff;
            }
        }
        raw
    }

    fn temp_path(name: &str) -> PathBuf {
        let mut path = env::temp_dir();
        path.push(format!("mister-tool-test-{name}-{}", unix_secs()));
        path
    }

    #[test]
    fn parses_relevant_ini_keys_with_sections_and_line_numbers() {
        let ini = r#"
; ignored
direct_video=1
[MiSTer]
direct_video=0
fb_terminal=1
fb_size=0
main=MiSTer_MagiK
[Menu]
video_mode=8
[arcade_vertical]
video_mode=14
"#;
        let parsed = parse_ini_keys(ini.to_string());
        assert_eq!(parsed["global"]["direct_video"]["value"], "1");
        assert_eq!(parsed["MiSTer"]["main"]["value"], "MiSTer_MagiK");
        assert_eq!(parsed["MiSTer"]["main"]["line"], 8);
        assert_eq!(parsed["Menu"]["video_mode"]["value"], "8");
        assert_eq!(parsed["arcade_vertical"]["video_mode"]["value"], "14");
        assert!(parsed["MiSTer"].get("unknown").is_none());
    }

    #[test]
    fn ini_parser_ignores_malformed_sections_and_comments() {
        let parsed = parse_ini_keys(
            "[MiSTer]\nmain=MiSTer_MagiK ; boot fork\n[broken\nvideo_mode=4\n# comment\n[Menu] ; inline note\nvideo_mode=8\n"
                .to_string(),
        );

        assert_eq!(
            parsed["MiSTer"]["main"]["value"],
            "MiSTer_MagiK ; boot fork"
        );
        assert_eq!(parsed["MiSTer"]["video_mode"]["value"], "4");
        assert_eq!(parsed["Menu"]["video_mode"]["value"], "8");
    }

    #[test]
    fn magik_boot_edit_sets_launcher_safe_video_without_touching_arcade_vertical() {
        let ini = "[MiSTer]\r\n; keep original core output for external scaler\r\ndirect_video=1\r\nmain=mister-magik-fb ; old handoff\r\n\r\n[arcade_vertical]\r\ndirect_video=0\r\nvideo_mode=14\r\nvscale_mode=1\r\n\r\n[Menu]\r\ndirect_video=0\r\nvideo_mode=4 ; menu probe\r\n";

        let edited = edit_mister_ini(ini, IniEdit::MagikBoot);

        assert!(edited.contains("direct_video=0\r\nmain=MiSTer_MagiK ; old handoff"));
        assert!(edited
            .contains("[arcade_vertical]\r\ndirect_video=0\r\nvideo_mode=14\r\nvscale_mode=1"));
        assert!(edited.contains("[Menu]\r\ndirect_video=0\r\nvideo_mode=8 ; menu probe"));
        assert!(edited.contains("; keep original core output for external scaler"));
    }

    #[test]
    fn magik_boot_edit_adds_menu_section_and_sets_global_direct_video_off() {
        let ini = "[MiSTer]\ndirect_video=1\n";
        let edited = edit_mister_ini(ini, IniEdit::MagikBoot);

        assert!(edited.contains("[MiSTer]\ndirect_video=0\nmain=MiSTer_MagiK"));
        assert!(edited.contains("[Menu]\ndirect_video=0\nvideo_mode=8"));
    }

    #[test]
    fn zaparoo_boot_edit_selects_zaparoo_fork_and_launcher_safe_video() {
        let ini = "[MiSTer]\r\nmain=MiSTer_MagiK ; current launcher\r\ndirect_video=1\r\n\r\n[Menu]\r\nvideo_mode=6\r\n";

        let edited = edit_mister_ini(ini, IniEdit::ZaparooBoot);

        assert!(edited.contains("main=zaparoo/MiSTer_Zaparoo ; current launcher\r\n"));
        assert!(edited.contains("direct_video=0\r\n"));
        assert!(edited.contains("[Menu]\r\nvideo_mode=8\r\n"));
    }

    #[test]
    fn local_probe_edits_use_preserving_mutator() {
        let ini = "[MiSTer]\nmain=MiSTer_MagiK\nforced_scandoubler=0\nmenu_pal=0\ndirect_video=1\n\n[Menu]\nvideo_mode=8\n";
        let crt = edit_mister_ini(
            ini,
            IniEdit::Crt {
                direct_video: "2".into(),
                menu_pal: "1".into(),
                forced_scandoubler: "1".into(),
            },
        );
        assert!(crt.contains("forced_scandoubler=1\nmenu_pal=1\ndirect_video=2"));

        let stock = edit_mister_ini(&crt, IniEdit::CommentMain);
        assert!(stock.contains(";main=MiSTer_MagiK ; MiSTer MagiK disabled for stock probe"));

        let auto = edit_mister_ini(&stock, IniEdit::MenuAuto);
        assert!(auto.contains(";video_mode=8 ; MiSTer MagiK EDID/native video-mode probe"));
    }

    #[test]
    fn stock_boot_restore_comments_only_magik_main_with_crlf_and_inline_comment() {
        let ini = "[MiSTer]\r\nmain=MiSTer_MagiK ; keep note\r\ndirect_video=1\r\n\r\n[Menu]\r\nvideo_mode=8\r\n";

        let edited = edit_mister_ini(ini, IniEdit::StockBoot);

        assert!(
            edited.contains(";main=MiSTer_MagiK ; keep note ; MiSTer MagiK stock boot restore\r\n")
        );
        assert!(edited.contains("direct_video=1\r\n"));
        assert!(edited.contains("[Menu]\r\nvideo_mode=8\r\n"));
    }

    #[test]
    fn stock_boot_restore_leaves_missing_or_unrelated_main_alone() {
        let missing = "[Menu]\nvideo_mode=8\n";
        assert_eq!(edit_mister_ini(missing, IniEdit::StockBoot), missing);

        let unrelated = "[MiSTer]\nmain=Some_Other_Menu\n";
        assert_eq!(edit_mister_ini(unrelated, IniEdit::StockBoot), unrelated);
    }

    #[test]
    fn stock_boot_restore_is_idempotent_for_commented_main() {
        let ini = "[MiSTer]\n;main=MiSTer_MagiK ; already disabled\n";
        assert_eq!(edit_mister_ini(ini, IniEdit::StockBoot), ini);
    }

    #[test]
    fn stock_boot_restore_comments_legacy_direct_slint_handoff() {
        let ini = "[MiSTer]\nmain=mister-magik-fb\n";
        let edited = edit_mister_ini(ini, IniEdit::StockBoot);

        assert!(edited.contains(";main=mister-magik-fb ; MiSTer MagiK stock boot restore"));
    }

    #[test]
    fn remote_run_rejects_removed_direct_arcade_scene() {
        assert!(validate_remote_run_command(
            "/media/fat/mister-magik/mister-magik-fb ui arcade 20"
        )
        .is_err());
        assert!(validate_remote_run_command(
            "'/media/fat/mister-magik/mister-magik-fb' ui arcade 20"
        )
        .is_err());
    }

    #[test]
    fn remote_run_allows_launcher_and_restart_paths() {
        assert!(validate_remote_run_command(
            "/media/fat/mister-magik/mister-magik-fb ui launcher 0"
        )
        .is_ok());
        assert!(validate_remote_run_command(
            "printf 'mister_magik_restart_launcher\\n' > /dev/MiSTer_cmd"
        )
        .is_ok());
    }

    #[test]
    fn reboot_remote_command_prefers_supervised_magik_command() {
        let cmd = reboot_remote_command(false);

        assert!(cmd.contains("mister_magik_reboot"));
        assert!(cmd.contains("/dev/MiSTer_cmd"));
        assert!(cmd.contains("MiSTer_MagiK"));
        assert!(!cmd.contains("/sbin/reboot"));
        assert!(cmd.contains("use --raw only for recovery"));
    }

    #[test]
    fn reboot_remote_command_raw_skips_supervised_command() {
        let cmd = reboot_remote_command(true);

        assert!(cmd.contains("/sbin/reboot"));
        assert!(!cmd.contains("mister_magik_reboot"));
    }

    #[test]
    fn raw_reboot_flag_is_removed_before_timeout_parse() {
        let mut args = vec!["--raw".to_string(), "180".to_string()];

        assert!(take_raw_reboot_flag(&mut args));
        assert_eq!(args, vec!["180"]);
        assert!(!take_raw_reboot_flag(&mut args));
    }

    #[test]
    fn status_prefers_launcher_process_over_helper_processes() {
        let status = json!({
            "processes": {
                "mister-magik-fb": [
                    {"pid": 1661, "cmdline": "/media/fat/mister-magik/mister-magik-fb library-refresh"},
                    {"pid": 1528, "cmdline": "/media/fat/mister-magik/mister-magik-fb ui launcher 0"}
                ]
            }
        });

        assert_eq!(
            primary_process(&status, "mister-magik-fb").and_then(|process| process["pid"].as_u64()),
            Some(1528)
        );
    }

    #[test]
    fn arcade_video_edit_sets_normal_direct_and_vertical_1080p() {
        let ini = "[MiSTer]\ndirect_video=0\nmain=MiSTer_MagiK\n\n[arcade_vertical]\ndirect_video=0\nvideo_mode=14\nvscale_mode=1\n";

        let edited = edit_mister_ini(ini, IniEdit::ArcadeVideo);

        assert!(edited.contains("[MiSTer]\ndirect_video=0\nmain=MiSTer_MagiK"));
        assert!(edited.contains("[arcade]\ndirect_video=1"));
        assert!(edited.contains("[arcade_vertical]\ndirect_video=0\nvideo_mode=8\nvscale_mode=1"));
        assert!(edited.find("[arcade]\n").unwrap() < edited.find("[arcade_vertical]\n").unwrap());
    }

    #[test]
    fn validates_local_ini_edit_argument_counts() {
        let args = vec!["menu-mode".into(), "8".into(), "in".into(), "out".into()];
        assert!(validate_ini_edit_local_args(&args).is_ok());

        let missing_mode = vec!["menu-mode".into(), "in".into(), "out".into()];
        assert!(validate_ini_edit_local_args(&missing_mode).is_err());

        let missing_crt_value = vec![
            "crt".into(),
            "1".into(),
            "0".into(),
            "in".into(),
            "out".into(),
        ];
        assert!(validate_ini_edit_local_args(&missing_crt_value).is_err());
    }

    #[test]
    fn filters_inittab_lines_by_needles() {
        let lines = lines_containing(
            "::sysinit:/media/fat/MiSTer &\n::respawn:/sbin/getty tty1\nboot.sh mister-magik\n"
                .to_string(),
            &["MiSTer", "mister-magik"],
        );
        assert_eq!(
            lines,
            vec![
                "::sysinit:/media/fat/MiSTer &".to_string(),
                "boot.sh mister-magik".to_string()
            ]
        );
    }

    #[test]
    fn parses_input_devices_into_names_handlers_and_ids() {
        let devices = parse_input_devices(
            r#"I: Bus=0003 Vendor=2563 Product=0575 Version=0111
N: Name="Retro-bit Controller"
H: Handlers=js0 event4

I: Bus=0003 Vendor=0000 Product=0000 Version=0004
N: Name="MiSTer virtual input"
H: Handlers=sysrq kbd event7
"#
            .to_string(),
        );
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0]["name"], "Retro-bit Controller");
        assert_eq!(devices[0]["handlers"], json!(["js0", "event4"]));
        assert_eq!(
            devices[1]["id"],
            "Bus=0003 Vendor=0000 Product=0000 Version=0004"
        );
    }

    #[test]
    fn parses_input_devices_without_trailing_blank_line() {
        let devices = parse_input_devices(
            r#"I: Bus=0003 Vendor=045e Product=028e Version=0114
N: Name="Xbox 360 Controller"
H: Handlers=event3 js0"#
                .to_string(),
        );

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0]["name"], "Xbox 360 Controller");
        assert_eq!(devices[0]["handlers"], json!(["event3", "js0"]));
    }

    #[test]
    fn classifies_black_slint_and_static_like_framebuffers() {
        let geometry = default_fb_geometry();
        let black = vec![0; geometry.bytes().unwrap()];
        assert_eq!(classify_fb(&black, &geometry)["class"], "mostly_black");

        let slint = raw_frame_with(|x, _| {
            if x < DEFAULT_FB_W / 2 {
                (0x06, 0xd6, 0xa0)
            } else {
                (0xe8, 0xe0, 0xf0)
            }
        });
        assert_eq!(classify_fb(&slint, &geometry)["class"], "slint_like");

        let static_like = raw_frame_with(|x, y| {
            if (x / 16 + y / 16) % 2 == 0 {
                (0xff, 0xff, 0xff)
            } else {
                (0x10, 0x10, 0x10)
            }
        });
        assert_eq!(classify_fb(&static_like, &geometry)["class"], "static_like");
    }

    #[test]
    fn parses_virtual_size() {
        assert_eq!(parse_virtual_size("960,540"), Some((960, 540)));
        assert_eq!(parse_virtual_size(" 1920,1080\n"), Some((1920, 1080)));
        assert_eq!(parse_virtual_size("bad"), None);
        assert_eq!(parse_virtual_size("960x540"), None);
        assert_eq!(parse_virtual_size("960,"), None);
    }

    #[test]
    fn framebuffer_geometry_bytes_detects_overflow() {
        let geometry = FbGeometry {
            width: 1,
            height: usize::MAX,
            stride: 2,
            bpp: 16,
        };

        assert!(geometry
            .bytes()
            .unwrap_err()
            .to_string()
            .contains("overflow"));
    }

    #[test]
    fn classifies_strided_960x540_framebuffer() {
        let geometry = FbGeometry {
            width: 960,
            height: 540,
            stride: 4096,
            bpp: 32,
        };
        let raw = raw_frame_with_geometry(geometry, |x, _| {
            if x < 480 {
                (0x06, 0xd6, 0xa0)
            } else {
                (0xe8, 0xe0, 0xf0)
            }
        });
        assert_eq!(raw.len(), 4096 * 540);
        assert_eq!(classify_fb(&raw, &geometry)["width"], 960);
        assert_eq!(classify_fb(&raw, &geometry)["height"], 540);
        assert_eq!(classify_fb(&raw, &geometry)["stride"], 4096);
        assert_eq!(classify_fb(&raw, &geometry)["class"], "slint_like");
    }

    #[test]
    fn doctor_reports_ok_for_nominal_launcher_state() {
        let findings = doctor_findings(&status_fixture());
        assert_eq!(
            findings,
            vec![(
                "ok".to_string(),
                "No obvious launcher/display problems found".to_string()
            )]
        );
    }

    #[test]
    fn doctor_reports_actionable_failures() {
        let mut status = status_fixture();
        status["boot"]["ini_keys"]["MiSTer"]["main"]["value"] = json!("mister-magik-fb");
        status["boot"]["ini_keys"]["MiSTer"]["direct_video"]["value"] = json!("1");
        status["boot"]["ini_keys"]["arcade"]["direct_video"]["value"] = json!("0");
        status["boot"]["ini_keys"]["Menu"]["direct_video"]["value"] = json!("1");
        status["boot"]["ini_keys"]["Menu"]["video_mode"]["value"] = json!("6");
        status["processes"]["mister-magik-fb"] = json!([]);
        status["display"]["active_vt"] = json!("tty1");
        status["display"]["fb0_visual"]["class"] = json!("mostly_black");
        status["runtime"]["main_status"]["visible_owner"] = json!("menu_bg");
        status["owners"]["by_device"]["/dev/fb0"] = json!([]);

        let findings = doctor_findings(&status);
        let texts: Vec<_> = findings.iter().map(|(_, text)| text.as_str()).collect();
        assert!(texts.contains(&"[MiSTer] main is not MiSTer_MagiK"));
        assert!(texts.contains(
            &"[MiSTer] direct_video is not 0; launcher boot may use direct-video timings"
        ));
        assert!(texts.contains(
            &"[arcade] direct_video is not 1; normal arcade games will use scaler output"
        ));
        assert!(texts.contains(&"[Menu] direct_video is not 0"));
        assert!(texts.contains(&"[Menu] video_mode is not 8"));
        assert!(texts.contains(&"mister-magik-fb is not running"));
        assert!(texts.contains(&"/dev/fb0 samples as mostly_black"));
        assert!(texts.contains(&"Main reports visible_owner=menu_bg rather than fb0"));
        assert!(texts.contains(&"/dev/fb0 is not owned by mister-magik-fb"));
    }

    #[test]
    fn doctor_reports_arcade_vertical_section_order_regression() {
        let mut status = status_fixture();
        status["boot"]["ini_keys"]["arcade"]["direct_video"]["line"] = json!(30);
        status["boot"]["ini_keys"]["arcade_vertical"]["direct_video"]["line"] = json!(20);

        let findings = doctor_findings(&status);
        let texts: Vec<_> = findings.iter().map(|(_, text)| text.as_str()).collect();
        assert!(texts.contains(
            &"[arcade] appears after [arcade_vertical]; vertical arcade settings will be overwritten"
        ));
    }

    #[test]
    fn display_read_requires_unsafe_spi_when_main_is_running() {
        let status = status_fixture();
        assert!(display_read_needs_unsafe_spi(&status));

        let mut no_main = status;
        no_main["processes"]["MiSTer_MagiK"] = json!([]);
        no_main["processes"]["MiSTer"] = json!([]);
        assert!(!display_read_needs_unsafe_spi(&no_main));
    }

    #[test]
    fn shell_quote_handles_single_quotes() {
        assert_eq!(sh("/tmp/simple"), "'/tmp/simple'");
        assert_eq!(sh("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn option_value_reads_next_arg() {
        let args = vec![
            "--settle".to_string(),
            "12".to_string(),
            "--keep-enabled".to_string(),
        ];
        assert_eq!(option_value(&args, "--settle"), Some("12".to_string()));
        assert_eq!(option_value(&args, "--missing"), None);
    }

    #[test]
    fn preview_cache_jobs_filter_media_and_reject_duplicate_stems() {
        let dir = temp_path("preview-cache-jobs");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("1942.PNG"), b"not decoded here").unwrap();
        fs::write(dir.join("pacman.jpeg"), b"not decoded here").unwrap();
        fs::write(dir.join("._pacman.jpeg"), b"resource fork").unwrap();
        fs::write(dir.join("notes.txt"), b"ignore").unwrap();
        fs::create_dir(dir.join("nested.png")).unwrap();

        let jobs = preview_cache_jobs(&dir).unwrap();
        let stems: Vec<_> = jobs.iter().map(|job| job.stem.as_str()).collect();
        assert_eq!(stems, vec!["1942", "pacman"]);

        fs::write(dir.join("pacman.jpg"), b"duplicate stem").unwrap();
        let err = preview_cache_jobs(&dir).unwrap_err().to_string();
        assert!(err.contains("duplicate source stem: pacman"));

        let _ = fs::remove_dir_all(dir);
    }

    #[derive(Debug)]
    struct TestArchiveEntry {
        name: String,
        raw_len: u32,
        payload_len: u32,
        offset: u64,
    }

    fn read_test_archive(path: &Path) -> (Vec<TestArchiveEntry>, Vec<u8>) {
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[..8], b"MMLZ4B1\0");
        let count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let mut pos = 12;
        let mut entries = Vec::new();
        for _ in 0..count {
            let name_len = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            let raw_len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let payload_len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let offset = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let name = String::from_utf8(bytes[pos..pos + name_len].to_vec()).unwrap();
            pos += name_len;
            entries.push(TestArchiveEntry {
                name,
                raw_len,
                payload_len,
                offset,
            });
        }
        (entries, bytes)
    }

    #[test]
    fn preview_archive_writer_sorts_entries_and_writes_runtime_format() {
        let dir = temp_path("preview-archive-writer");
        let _ = fs::remove_dir_all(&dir);
        let raw_dir = dir.join("raw565-hybrid-320x320");
        fs::create_dir_all(&raw_dir).unwrap();
        fs::write(raw_dir.join("z-last.rgb565"), b"x").unwrap();
        fs::write(raw_dir.join("a-first.rgb565"), vec![0u8; 256]).unwrap();

        let archive_path = dir.join("raw565-hybrid-320x320-lz4block-12.mmlz4b");
        let summary = build_preview_archive(&raw_dir, &archive_path).unwrap();
        assert_eq!(summary.entries, 2);
        assert_eq!(summary.raw_bytes, 257);
        assert!(summary.archive_bytes > 0);

        let (entries, bytes) = read_test_archive(&archive_path);
        assert_eq!(entries[0].name, "a-first.rgb565");
        assert_eq!(entries[1].name, "z-last.rgb565");
        assert_eq!(entries[0].raw_len, 256);
        assert_eq!(entries[1].raw_len, 1);

        let index_len = 12
            + entries
                .iter()
                .map(|entry| 2 + 4 + 4 + 8 + entry.name.len())
                .sum::<usize>();
        assert_eq!(entries[0].offset, index_len as u64);
        assert_eq!(
            entries[1].offset,
            entries[0].offset + entries[0].payload_len as u64
        );
        assert_eq!(bytes[entries[0].offset as usize], 0);
        assert_eq!(bytes[entries[1].offset as usize], 1);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn preview_cache_build_writes_raw565_and_archive_in_one_job() {
        let dir = temp_path("preview-cache-build-archive");
        let _ = fs::remove_dir_all(&dir);
        let input = dir.join("input");
        let output = dir.join("cache");
        fs::create_dir_all(&input).unwrap();

        RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]))
            .save(input.join("b-title.png"))
            .unwrap();
        RgbImage::from_pixel(4, 1, image::Rgb([0, 255, 0]))
            .save(input.join("a-title.png"))
            .unwrap();

        preview_cache_build(&[
            "--input".into(),
            input.display().to_string(),
            "--output".into(),
            output.display().to_string(),
            "--max".into(),
            "3".into(),
        ])
        .unwrap();

        assert!(output
            .join("raw565-hybrid-3x3")
            .join("a-title.rgb565")
            .exists());
        assert!(output
            .join("raw565-hybrid-3x3")
            .join("b-title.rgb565")
            .exists());
        let archive_path = output.join("raw565-hybrid-3x3-lz4block-12.mmlz4b");
        assert!(archive_path.exists());
        let (entries, _) = read_test_archive(&archive_path);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a-title.rgb565", "b-title.rgb565"]
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn png_writer_outputs_valid_signature_and_chunks() {
        let path = temp_path("tiny.png");
        let raw = vec![
            0x00, 0x00, 0xff, 0x00, // red in BGRX
            0x00, 0xff, 0x00, 0x00, // green
            0xff, 0x00, 0x00, 0x00, // blue
            0xff, 0xff, 0xff, 0x00, // white
        ];
        write_png_bgrx(&raw, 2, 2, &path).unwrap();
        let png = fs::read(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(png.windows(4).any(|w| w == b"IHDR"));
        assert!(png.windows(4).any(|w| w == b"IDAT"));
        assert!(png.windows(4).any(|w| w == b"IEND"));
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
    }

    #[test]
    fn raw_to_png_reads_bgrx_file_and_rejects_short_input() {
        let raw_path = temp_path("tiny.raw");
        let png_path = temp_path("tiny-from-raw.png");
        fs::write(
            &raw_path,
            [
                0x00, 0x00, 0xff, 0x00, // red in BGRX
                0x00, 0xff, 0x00, 0x00, // green
                0xff, 0x00, 0x00, 0x00, // blue
                0xff, 0xff, 0xff, 0x00, // white
            ],
        )
        .unwrap();
        raw_to_png(&raw_path, 2, 2, &png_path).unwrap();
        let png = fs::read(&png_path).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));

        let err = raw_to_png(&raw_path, 3, 3, &png_path).unwrap_err();
        assert!(err.to_string().contains("expected at least 36"));
        let _ = fs::remove_file(&raw_path);
        let _ = fs::remove_file(&png_path);
    }

    #[test]
    fn preview_target_size_matches_arcade_cache_policy() {
        assert_eq!(preview_target_size(224, 256, 320), Some((280, 320, 1.25)));
        let downscale = preview_target_size(384, 224, 320).unwrap();
        assert_eq!((downscale.0, downscale.1), (320, 187));
        assert!(downscale.2 < 1.0);
        assert_eq!(preview_target_size(320, 240, 320), None);
        assert_eq!(
            preview_target_size(224, 384, 320).map(|v| (v.0, v.1)),
            Some((187, 320))
        );
    }

    #[test]
    fn preview_resize_image_chooses_filter_by_scale_direction() {
        let small = RgbImage::from_pixel(2, 1, image::Rgb([255, 0, 0]));
        let (upscaled, up_filter) = resize_preview_image(small, 4);
        assert_eq!((upscaled.width(), upscaled.height()), (4, 2));
        assert_eq!(up_filter, PreviewResizeChoice::Nearest);
        assert_eq!(up_filter.label(), "nearest");

        let large = RgbImage::from_pixel(8, 4, image::Rgb([0, 255, 0]));
        let (downscaled, down_filter) = resize_preview_image(large, 4);
        assert_eq!((downscaled.width(), downscaled.height()), (4, 2));
        assert_eq!(down_filter, PreviewResizeChoice::Lanczos);
        assert_eq!(down_filter.label(), "lanczos");

        let exact = RgbImage::from_pixel(4, 2, image::Rgb([0, 0, 255]));
        let (unchanged, unchanged_filter) = resize_preview_image(exact, 4);
        assert_eq!((unchanged.width(), unchanged.height()), (4, 2));
        assert_eq!(unchanged_filter, PreviewResizeChoice::Unchanged);
        assert_eq!(unchanged_filter.label(), "unchanged");
    }

    #[test]
    fn parses_mame_1942_metadata() {
        let machines = parse_mame_listxml(MAME_1942_FIXTURE).unwrap();
        let parent = machines
            .iter()
            .find(|machine| machine.setname == "1942")
            .unwrap();
        let clone = machines
            .iter()
            .find(|machine| machine.setname == "1942a")
            .unwrap();

        assert_eq!(parent.parent_setname, None);
        assert_eq!(parent.title, "1942 (Revision B)");
        assert_eq!(parent.year.as_deref(), Some("1984"));
        assert_eq!(parent.manufacturer.as_deref(), Some("Capcom"));
        assert_eq!(parent.rotate, Some(270));
        assert_eq!(parent.display_width, Some(256));
        assert_eq!(parent.display_height, Some(224));
        assert_eq!(parent.players, Some(2));
        assert_eq!(parent.coins, Some(2));
        assert_eq!(parent.control_type.as_deref(), Some("joy"));
        assert_eq!(parent.control_ways.as_deref(), Some("8"));
        assert_eq!(parent.buttons, Some(2));
        assert_eq!(parent.driver_status.as_deref(), Some("good"));
        assert_eq!(parent.source_version, "0.288 (mame0288)");
        assert_eq!(clone.parent_setname.as_deref(), Some("1942"));
    }

    #[test]
    fn writes_mame_metadata_sqlite() {
        let machines = parse_mame_listxml(MAME_1942_FIXTURE).unwrap();
        let path = temp_path("mame.sqlite3");
        write_mame_metadata_db(&path, &machines, &[], &[]).unwrap();
        let conn = Connection::open(&path).unwrap();
        let row: (String, String, i64, i64) = conn
            .query_row(
                "SELECT parent_setname, manufacturer, rotate, buttons FROM mame_machines WHERE setname='1942a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(row, ("1942".to_string(), "Capcom".to_string(), 270, 2));
    }

    #[test]
    fn loads_mame_machines_from_existing_sqlite() {
        let machines = parse_mame_listxml(MAME_1942_FIXTURE).unwrap();
        let path = temp_path("mame-machine-source.sqlite3");
        write_mame_metadata_db(&path, &machines, &[], &[]).unwrap();
        let loaded = load_mame_machines_from_db(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert!(loaded.iter().any(|machine| {
            machine.setname == "1942a"
                && machine.parent_setname.as_deref() == Some("1942")
                && machine.buttons == Some(2)
        }));
    }

    #[test]
    fn parses_mame_software_list_items_and_hashes() {
        let (items, hashes) = parse_mame_software_list_xml(
            r#"
            <softwarelist name="saturn" description="Saturn">
              <software name="nights" cloneof="nightsu">
                <description>Nights into Dreams (Europe)</description>
                <year>1996</year>
                <publisher>Sega</publisher>
                <part name="cdrom" interface="saturn_cdrom">
                  <diskarea name="cdrom">
                    <disk name="nights" sha1="ABCDEF0123456789ABCDEF0123456789ABCDEF01"/>
                  </diskarea>
                </part>
              </software>
              <software name="sonic">
                <description>Sonic the Hedgehog (USA)</description>
                <year>1991</year>
                <publisher>Sega</publisher>
                <part name="cart" interface="megadriv_cart">
                  <dataarea name="rom" size="524288">
                    <rom name="sonic.bin" size="524288" crc="F9394E97" sha1="0123456789ABCDEF0123456789ABCDEF01234567"/>
                  </dataarea>
                </part>
              </software>
            </softwarelist>
            "#,
        )
        .unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].list_name, "saturn");
        assert_eq!(items[0].software_name, "nights");
        assert_eq!(items[0].parent_name.as_deref(), Some("nightsu"));
        assert_eq!(items[0].region.as_deref(), Some("europe"));
        assert_eq!(hashes.len(), 2);
        assert_eq!(
            hashes[0].disk_sha1.as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef01")
        );
        assert_eq!(hashes[1].crc32.as_deref(), Some("f9394e97"));
        assert_eq!(
            hashes[1].sha1.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
    }

    #[test]
    fn raw565_preview_header_and_stride_match_runtime_format() {
        let image = RgbImage::from_raw(
            3,
            1,
            vec![
                255, 0, 0, // red
                0, 255, 0, // green
                0, 0, 255, // blue
            ],
        )
        .unwrap();
        let bytes = encode_raw565_preview(&image);
        assert_eq!(&bytes[..8], b"MM56501\0");
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 16);
        assert_eq!(bytes.len(), 20 + 16);
        assert_eq!(
            &bytes[20..26],
            &[
                0x00, 0xf8, // red
                0xe0, 0x07, // green
                0x1f, 0x00, // blue
            ]
        );
        assert!(bytes[26..].iter().all(|b| *b == 0));
    }

    #[test]
    fn profile_summary_reports_frame_count_and_percentiles() {
        let path = temp_path("profile.tsv");
        fs::write(
            &path,
            "frame\twall_us\trender_us\tcopy_us\n0\t10\t100\t7\n1\t20\t200\t9\n2\t30\t300\t11\n",
        )
        .unwrap();
        let text = profile_summary_text(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert!(text.contains("(3 frames)"));
        assert!(text.contains("wall_us"));
        assert!(text.contains("min=    10"));
        assert!(text.contains("p50=    20"));
        assert!(text.contains("render_us"));
        assert!(text.contains("copy_us"));
    }

    const MAME_1942_FIXTURE: &str = r#"<?xml version="1.0"?>
<mame build="0.288 (mame0288)" debug="no" mameconfig="10">
  <machine name="1942" sourcefile="capcom/1942.cpp">
    <description>1942 (Revision B)</description>
    <year>1984</year>
    <manufacturer>Capcom</manufacturer>
    <display tag="screen" type="raster" rotate="270" width="256" height="224" refresh="59.637405" />
    <input players="2" coins="2">
      <control type="joy" player="1" buttons="2" ways="8" />
      <control type="joy" player="2" buttons="2" ways="8" />
    </input>
    <driver status="good" emulation="good" savestate="supported" />
  </machine>
  <machine name="1942a" sourcefile="capcom/1942.cpp" cloneof="1942" romof="1942">
    <description>1942 (Revision A)</description>
    <year>1984</year>
    <manufacturer>Capcom</manufacturer>
    <display tag="screen" type="raster" rotate="270" width="256" height="224" refresh="59.637405" />
    <input players="2" coins="2">
      <control type="joy" player="1" buttons="2" ways="8" />
      <control type="joy" player="2" buttons="2" ways="8" />
    </input>
    <driver status="good" emulation="good" savestate="supported" />
  </machine>
  <machine name="1942p" sourcefile="capcom/1942.cpp" cloneof="1942" romof="1942">
    <description>1942 (Tecfri PCB, bootleg?)</description>
    <year>1984</year>
    <manufacturer>bootleg</manufacturer>
    <display tag="screen" type="raster" rotate="270" width="256" height="224" refresh="59.637405" />
    <input players="1" coins="2">
      <control type="joy" buttons="2" ways="8" />
    </input>
    <driver status="good" emulation="good" savestate="supported" />
  </machine>
</mame>
"#;
}
