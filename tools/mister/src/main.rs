use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};
use ssh2::{ExtendedData, Session};
use std::collections::HashMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod media;

const DEFAULT_FB_W: usize = 1920;
const DEFAULT_FB_H: usize = 1080;
const DEFAULT_FB_BPP: usize = 32;
const AGENT_PORT: u16 = 7498;
const AGENT_TOKEN_LOCAL: &str = "build/mister-agent.token";
const AGENT_DEPLOY_COMPRESS_MIN_BYTES: usize = 8 * 1024 * 1024;
const RAW_REBOOT_REMOTE_CMD: &str = "nohup /sbin/reboot >/dev/null 2>&1 & echo raw";
const SUPERVISED_REBOOT_REMOTE_CMD: &str = "if [ -p /dev/MiSTer_cmd ] && pidof MiSTer_MagiK >/dev/null 2>&1; then printf 'mister_magik_reboot\\n' > /dev/MiSTer_cmd; echo supervised; else echo 'supervised reboot unavailable: MiSTer_MagiK or /dev/MiSTer_cmd missing' >&2; exit 12; fi";
const DEFAULT_REMOTE_LIBRARY_DB: &str = "/media/fat/mister-magik/library.sqlite3";
const DEFAULT_LAUNCHER_ENV_REMOTE: &str = "/media/fat/mister-magik/launcher.env";
const MAIN_STATUS_REMOTE: &str = "/tmp/mister-magik/main-status.json";
const SLINT_STATUS_REMOTE: &str = "/tmp/mister-magik/status.json";

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
        "deploy-magik-bin" => {
            if args.is_empty() {
                return Err("deploy-magik-bin needs <local> [remote]".into());
            }
            let remote = args
                .get(1)
                .map(String::as_str)
                .unwrap_or("/media/fat/mister-magik/mister-magik-fb");
            let sess = connect(10)?;
            deploy_magik_bin(&sess, Path::new(&args[0]), remote)?;
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
        "connection-profile" => {
            connection_profile(&args)?;
        }
        "media-check" => {
            if media::media_help_requested(&args) {
                media::media_usage();
                return Ok(());
            }
            let sess = connect(10)?;
            media::media_check(&sess, &args)?;
        }
        "media-download" => {
            if media::media_help_requested(&args) {
                media::media_usage();
                return Ok(());
            }
            let sess = connect(10)?;
            media::media_download(&sess, &args)?;
        }
        "media-bench-download" => {
            if media::media_help_requested(&args) {
                media::media_usage();
                return Ok(());
            }
            let sess = connect(10)?;
            media::media_bench_download(&sess, &args)?;
        }
        "media-cloudflare-check" => {
            media::media_cloudflare_check(&args)?;
        }
        "launcher-restart" => {
            if launcher_restart_help_requested(&args) {
                launcher_restart_usage();
                return Ok(());
            }
            let options = parse_launcher_restart_args(&args)?;
            let sess = connect(10)?;
            launcher_restart(&sess, &options)?;
        }
        "boot-net-profile" => {
            boot_net_profile(&args)?;
        }
        "boot-tcp-profile" => {
            boot_tcp_profile(&args)?;
        }
        "agent" => {
            agent_cli(&args)?;
        }
        "watch-reboot" => {
            watch_external_reboot(&args)?;
        }
        "reboot" | "reboot-wait" => {
            let raw = take_reboot_raw_flag(&mut args)?;
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
        "inittab-ensure-stock" => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let sess = connect(10)?;
            ensure_stock_inittab(&sess, dry_run)?;
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
            raw_to_png_cli(&args)?;
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
        "usage: scripts/mister <run|put|deploy-magik-bin|get|db|library-db|wait|connection-profile|media-check|media-download|media-bench-download|media-cloudflare-check|launcher-restart|boot-net-profile|boot-tcp-profile|agent|watch-reboot|reboot|reboot-wait|status|doctor|snapshot|boot-capture|display-read|ini-repair-boot|inittab-ensure-stock|ini-restore-stock|ini-zaparoo-boot|ini-edit-local|profile-summary|raw-to-png|mame-metadata-build|recover> ...\n       mame-metadata-build --out <sqlite> [--listxml <xml>|--mame <bin>|--machine-sqlite <sqlite>] [--category-ini <ini>|--catver-ini <ini>]...\n       launcher-restart [--env KEY=VALUE]... [--clear-env] [--timeout SECS]; agent <ping|status|logs|boot-profile>; reboot/reboot-wait default to supervised MagiK visual-lockdown reboot; pass --raw for detached Linux reboot recovery"
    );
}

fn take_reboot_raw_flag(args: &mut Vec<String>) -> Result<bool> {
    let raw = if let Some(pos) = args.iter().position(|arg| arg == "--raw") {
        args.remove(pos);
        true
    } else {
        false
    };
    let supervised = if let Some(pos) = args.iter().position(|arg| arg == "--supervised") {
        args.remove(pos);
        true
    } else {
        false
    };
    if raw && supervised {
        Err("use only one of --raw or --supervised".into())
    } else {
        Ok(raw)
    }
}

fn reboot_raw_from_args(args: &[String]) -> Result<bool> {
    let raw = args.iter().any(|arg| arg == "--raw");
    let supervised = args.iter().any(|arg| arg == "--supervised");
    if raw && supervised {
        Err("use only one of --raw or --supervised".into())
    } else {
        Ok(raw)
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
    category: Option<String>,
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
    let mut machines = if let Some(machine_sqlite) = option_value(args, "--machine-sqlite") {
        load_mame_machines_from_db(Path::new(&machine_sqlite))?
    } else {
        let xml = if let Some(listxml) = option_value(args, "--listxml") {
            fs::read_to_string(listxml)?
        } else {
            let mame = option_value(args, "--mame")
            .or_else(|| env::var("MAME_BIN").ok())
            .or_else(|| find_program_on_path("mame"))
            .ok_or("mame-metadata-build needs --listxml <mame-listxml>, --mame <binary>, MAME_BIN, or mame on PATH")?;
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
    let categories = load_mame_category_ini_files(args)?;
    if !categories.is_empty() {
        for machine in &mut machines {
            machine.category = categories.get(&machine.setname).cloned();
        }
    }
    let (software_items, software_hashes) = load_mame_software_list_xmls(args)?;
    write_mame_metadata_db(
        Path::new(&out),
        &machines,
        &software_items,
        &software_hashes,
    )?;
    println!(
        "mame_metadata_build out={} machines={} categories={} software_items={} software_hashes={} source_version={}",
        out,
        machines.len(),
        categories.len(),
        software_items.len(),
        software_hashes.len(),
        machines
            .first()
            .map(|machine| machine.source_version.as_str())
            .unwrap_or("unknown")
    );
    Ok(())
}

fn load_mame_category_ini_files(args: &[String]) -> Result<HashMap<String, String>> {
    let mut out = HashMap::<String, String>::new();
    let paths = option_values(args, "--category-ini")
        .into_iter()
        .chain(option_values(args, "--catver-ini"))
        .collect::<Vec<_>>();
    for path in paths {
        let text = fs::read_to_string(&path)?;
        out.extend(parse_mame_category_ini(&text));
    }
    Ok(out)
}

fn parse_mame_category_ini(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut in_category = false;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_category = line[1..line.len() - 1].eq_ignore_ascii_case("Category");
            continue;
        }
        if !in_category {
            continue;
        }
        let Some((setname, category)) = line.split_once('=') else {
            continue;
        };
        let setname = setname.trim();
        let category = category.trim();
        if !setname.is_empty() && !category.is_empty() {
            out.insert(setname.to_string(), category.to_string());
        }
    }
    out
}

fn load_mame_software_list_xmls(
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
    let category_expr = if sqlite_column_exists(&conn, "mame_machines", "category")? {
        "category"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT setname,parent_setname,title,year,manufacturer,sourcefile,rotate,display_type,
                display_width,display_height,refresh_hz,players,coins,control_type,control_ways,
                buttons,driver_status,emulation_status,savestate,source_version,{category_expr}
         FROM mame_machines
         ORDER BY setname"
    );
    let mut stmt = conn.prepare(&sql)?;
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
            category: row.get(20)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn sqlite_column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
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
            category TEXT,
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
                buttons,driver_status,emulation_status,savestate,category,source_version
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
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
                machine.category,
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
    let bytes = fs::read(local)?;
    put_bytes_with_sftp(&sftp, remote, &bytes)
}

fn put_bytes(sess: &Session, remote: &str, bytes: &[u8]) -> Result<()> {
    let sftp = sess.sftp()?;
    put_bytes_with_sftp(&sftp, remote, bytes)
}

fn put_bytes_with_sftp(sftp: &ssh2::Sftp, remote: &str, bytes: &[u8]) -> Result<()> {
    let mut dst = sftp.create(Path::new(remote))?;
    dst.write_all(bytes)?;
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

fn deploy_magik_bin(sess: &Session, local: &Path, remote: &str) -> Result<()> {
    let total_t = Instant::now();
    let validate_t = Instant::now();
    let transaction = MagikDeployTransaction::validate(local, remote)?;
    let validate_ms = validate_t.elapsed().as_millis();
    let report = transaction.run_ssh(sess, validate_ms, total_t)?;
    report.print();
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MagikDeployTransaction {
    local: PathBuf,
    remote: String,
    remote_dir: String,
    upload: String,
    lock: String,
    local_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MagikDeployReport {
    local: PathBuf,
    remote: String,
    local_bytes: u64,
    remote_bytes: u64,
    total_ms: u128,
    validate_ms: u128,
    prepare_ms: u128,
    suspend_ms: u128,
    upload_ms: u128,
    swap_ms: u128,
    chmod_size_ms: u128,
    resume_ms: u128,
    cleanup_ms: u128,
}

impl MagikDeployTransaction {
    fn validate(local: &Path, remote: &str) -> Result<Self> {
        if !remote.starts_with('/') || remote.ends_with('/') || remote.contains('\0') {
            return Err(format!("unsupported deploy remote: {remote}").into());
        }
        let remote_dir = remote_parent_dir(remote)?.to_string();
        let local_bytes = fs::metadata(local)?.len();
        Ok(Self {
            local: local.to_path_buf(),
            remote: remote.to_string(),
            upload: format!("{remote}.upload"),
            lock: format!("{remote_dir}/deploy.lock"),
            remote_dir,
            local_bytes,
        })
    }

    fn run_ssh(
        &self,
        sess: &Session,
        validate_ms: u128,
        total_t: Instant,
    ) -> Result<MagikDeployReport> {
        let prepare_ms = self.prepare(sess)?;
        let mut suspended = false;
        let mut cleaned = false;
        let result = (|| -> Result<MagikDeployReport> {
            let suspend_t = Instant::now();
            magik_fifo_command(sess, "mister_magik_suspend")?;
            let suspend_ms = suspend_t.elapsed().as_millis();
            suspended = true;

            let upload_t = Instant::now();
            put(sess, &self.local, &self.upload)?;
            let upload_ms = upload_t.elapsed().as_millis();

            let swap_ms = self.swap_upload(sess)?;
            let (chmod_size_ms, remote_bytes) = self.chmod_and_verify_size(sess)?;

            let resume_t = Instant::now();
            magik_fifo_command(sess, "mister_magik_resume")?;
            let resume_ms = resume_t.elapsed().as_millis();
            suspended = false;

            let cleanup_ms = self.cleanup(sess)?;
            cleaned = true;

            Ok(MagikDeployReport {
                local: self.local.clone(),
                remote: self.remote.clone(),
                local_bytes: self.local_bytes,
                remote_bytes,
                total_ms: total_t.elapsed().as_millis(),
                validate_ms,
                prepare_ms,
                suspend_ms,
                upload_ms,
                swap_ms,
                chmod_size_ms,
                resume_ms,
                cleanup_ms,
            })
        })();

        if result.is_err() {
            if !cleaned {
                let _ = self.cleanup(sess);
            }
            if suspended {
                let _ = magik_fifo_command(sess, "mister_magik_resume");
            }
        }
        result
    }

    fn prepare(&self, sess: &Session) -> Result<u128> {
        let start = Instant::now();
        self.exec_phase(
            sess,
            "prepare",
            &format!("mkdir -p {}; : > {}", sh(&self.remote_dir), sh(&self.lock)),
        )?;
        Ok(start.elapsed().as_millis())
    }

    fn swap_upload(&self, sess: &Session) -> Result<u128> {
        let start = Instant::now();
        self.exec_phase(
            sess,
            "swap",
            &format!("mv {} {}", sh(&self.upload), sh(&self.remote)),
        )?;
        Ok(start.elapsed().as_millis())
    }

    fn chmod_and_verify_size(&self, sess: &Session) -> Result<(u128, u64)> {
        let start = Instant::now();
        let out = self.exec_phase(sess, "chmod-size-verify", &self.chmod_size_verify_command())?;
        let remote_bytes = parse_wc_byte_count(&out.stdout)
            .ok_or_else(|| format!("unable to parse deployed size from: {}", out.stdout.trim()))?;
        if remote_bytes != self.local_bytes {
            return Err(format!(
                "deployed size mismatch local={} remote={}",
                self.local_bytes, remote_bytes
            )
            .into());
        }
        Ok((start.elapsed().as_millis(), remote_bytes))
    }

    fn chmod_size_verify_command(&self) -> String {
        format!(
            "chmod +x {} && wc -c {}",
            sh(&self.remote),
            sh(&self.remote)
        )
    }

    fn cleanup(&self, sess: &Session) -> Result<u128> {
        let start = Instant::now();
        self.exec_phase(
            sess,
            "cleanup",
            &format!("rm -f {} {}", sh(&self.upload), sh(&self.lock)),
        )?;
        Ok(start.elapsed().as_millis())
    }

    fn exec_phase(&self, sess: &Session, phase: &str, command: &str) -> Result<ExecOutput> {
        let out = exec(sess, command, true)?;
        if out.rc != 0 {
            return Err(format!(
                "deploy {phase} phase failed rc={} output={}",
                out.rc,
                out.stdout.trim()
            )
            .into());
        }
        Ok(out)
    }
}

impl MagikDeployReport {
    fn print(&self) {
        let finish_ms = self.swap_ms + self.chmod_size_ms;
        let resume_size_ms = self.resume_ms + self.chmod_size_ms;
        println!(
            "deploy_magik_bin local={} remote={} local_bytes={} remote_bytes={} total_ms={} prepare_ms={} suspend_ms={} put_ms={} finish_ms={} resume_size_ms={} validate_ms={} upload_ms={} swap_ms={} chmod_size_ms={} resume_ms={} cleanup_ms={}",
            self.local.display(),
            self.remote,
            self.local_bytes,
            self.remote_bytes,
            self.total_ms,
            self.prepare_ms,
            self.suspend_ms,
            self.upload_ms,
            finish_ms,
            resume_size_ms,
            self.validate_ms,
            self.upload_ms,
            self.swap_ms,
            self.chmod_size_ms,
            self.resume_ms,
            self.cleanup_ms
        );
    }
}

fn parse_wc_byte_count(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse::<u64>().ok()
}

fn verify_agent_deploy_result(
    result: &Value,
    expected_bytes: u64,
    expected_remote: &str,
) -> Result<u64> {
    let remote = result.get("remote").and_then(Value::as_str).unwrap_or("");
    if remote != expected_remote {
        return Err(format!(
            "agent deploy remote mismatch expected={expected_remote} actual={remote}"
        )
        .into());
    }
    let remote_bytes = result
        .get("remote_bytes")
        .and_then(Value::as_u64)
        .ok_or("agent deploy response missing remote_bytes")?;
    if remote_bytes != expected_bytes {
        return Err(format!(
            "agent deployed size mismatch expected={expected_bytes} remote={remote_bytes}"
        )
        .into());
    }
    Ok(remote_bytes)
}

fn magik_fifo_command(sess: &Session, command: &str) -> Result<()> {
    let out = exec(
        sess,
        &format!(
            "if [ -p /dev/MiSTer_cmd ] && pidof MiSTer_MagiK >/dev/null 2>&1; then printf '{}\\n' > /dev/MiSTer_cmd; fi",
            command
        ),
        true,
    )?;
    if out.rc == 0 {
        Ok(())
    } else {
        Err(format!("MiSTer command failed: {command}").into())
    }
}

struct TimedSession {
    sess: Session,
    resolve_ms: u128,
    tcp_ms: u128,
    handshake_ms: u128,
    auth_ms: u128,
}

fn connect_timed(timeout_secs: u64) -> Result<TimedSession> {
    let resolve_t = Instant::now();
    let addr = format!("{}:22", host())
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer host")?;
    let resolve_ms = resolve_t.elapsed().as_millis();

    let tcp_t = Instant::now();
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(timeout_secs))?;
    let tcp_ms = tcp_t.elapsed().as_millis();
    tcp.set_read_timeout(Some(Duration::from_secs(timeout_secs)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(timeout_secs)))?;

    let mut sess = Session::new()?;
    sess.set_tcp_stream(tcp);
    let handshake_t = Instant::now();
    sess.handshake()?;
    let handshake_ms = handshake_t.elapsed().as_millis();
    let auth_t = Instant::now();
    sess.userauth_password(&user(), &pass())?;
    let auth_ms = auth_t.elapsed().as_millis();
    if !sess.authenticated() {
        return Err("SSH password authentication failed".into());
    }
    Ok(TimedSession {
        sess,
        resolve_ms,
        tcp_ms,
        handshake_ms,
        auth_ms,
    })
}

fn append_profile_row(path: &str, header: &str, row: &str) -> Result<()> {
    let path = Path::new(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let needs_header = !path.exists() || path.metadata()?.len() == 0;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    if needs_header {
        writeln!(file, "{header}")?;
    }
    writeln!(file, "{row}")?;
    Ok(())
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn bytes_for_profile(size: usize) -> Vec<u8> {
    let mut x = 0x4d49_5354_4552_4d47u64;
    let mut bytes = Vec::with_capacity(size);
    while bytes.len() < size {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    bytes.truncate(size);
    bytes
}

fn fnv64_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn parse_profile_count(args: &[String], default: usize) -> usize {
    args.iter()
        .find(|arg| !arg.starts_with('-'))
        .and_then(|arg| arg.parse::<usize>().ok())
        .unwrap_or(default)
}

fn parse_profile_bytes(args: &[String], default: usize) -> usize {
    option_value(args, "--bytes")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(default)
}

fn sftp_write_profile(sess: &Session, remote: &str, bytes: &[u8]) -> Result<u128> {
    let sftp = sess.sftp()?;
    let t = Instant::now();
    let mut dst = sftp.create(Path::new(remote))?;
    dst.write_all(bytes)?;
    Ok(t.elapsed().as_millis())
}

fn connection_profile(args: &[String]) -> Result<()> {
    let samples = parse_profile_count(args, 5);
    let bytes_len = parse_profile_bytes(args, 4 * 1024 * 1024);
    let out_path = "history/toolchain-bench/results-connection-profile.tsv";
    let header = "kind\tts_unix_ms\tsample\thost\tbytes\tresolve_ms\ttcp_ms\thandshake_ms\tauth_ms\texec_ms\tsftp_init_ms\tput_tmp_ms\tput_tmp_mib_s\tput_fat_ms\tput_fat_mib_s\tuptime\tnote";
    println!("{header}");
    let bytes = bytes_for_profile(bytes_len);
    for sample in 1..=samples {
        let ts = unix_ms_now();
        match connect_timed(10) {
            Ok(timed) => {
                let exec_t = Instant::now();
                let uptime_out = exec(&timed.sess, "cat /proc/uptime", true)?;
                let exec_ms = exec_t.elapsed().as_millis();
                let uptime = uptime_out.stdout.split_whitespace().next().unwrap_or("");
                let sftp_t = Instant::now();
                let _ = timed.sess.sftp()?;
                let sftp_init_ms = sftp_t.elapsed().as_millis();
                let tag = format!("{}-{sample}-{ts}", std::process::id());
                let tmp_remote = format!("/tmp/mister-magik-profile-{tag}.bin");
                let fat_remote = format!("/media/fat/mister-magik/profile-tmp-{tag}.bin");
                let put_tmp_ms = sftp_write_profile(&timed.sess, &tmp_remote, &bytes)?;
                let _ = exec(
                    &timed.sess,
                    "mkdir -p /media/fat/mister-magik >/dev/null 2>&1 || true",
                    true,
                );
                let put_fat_ms = sftp_write_profile(&timed.sess, &fat_remote, &bytes)?;
                let _ = exec(
                    &timed.sess,
                    &format!("rm -f {} {}", sh(&tmp_remote), sh(&fat_remote)),
                    true,
                );
                let mib = bytes_len as f64 / (1024.0 * 1024.0);
                let tmp_mib_s = if put_tmp_ms > 0 {
                    mib * 1000.0 / put_tmp_ms as f64
                } else {
                    0.0
                };
                let fat_mib_s = if put_fat_ms > 0 {
                    mib * 1000.0 / put_fat_ms as f64
                } else {
                    0.0
                };
                let row = format!(
                    "connection\t{ts}\t{sample}\t{}\t{bytes_len}\t{}\t{}\t{}\t{}\t{exec_ms}\t{sftp_init_ms}\t{put_tmp_ms}\t{tmp_mib_s:.2}\t{put_fat_ms}\t{fat_mib_s:.2}\t{uptime}\tok",
                    host(),
                    timed.resolve_ms,
                    timed.tcp_ms,
                    timed.handshake_ms,
                    timed.auth_ms
                );
                println!("{row}");
                append_profile_row(out_path, header, &row)?;
            }
            Err(err) => {
                let row = format!(
                    "connection\t{ts}\t{sample}\t{}\t{bytes_len}\t\t\t\t\t\t\t\t\t\t\t\tERROR: {err}",
                    host()
                );
                println!("{row}");
                append_profile_row(out_path, header, &row)?;
            }
        }
        thread::sleep(Duration::from_millis(300));
    }
    eprintln!("connection-profile: appended {samples} row(s) to {out_path}");
    Ok(())
}

struct AgentResponse {
    response: Value,
    elapsed_ms: u128,
}

fn agent_cli(args: &[String]) -> Result<()> {
    let subcommand = args.first().map(String::as_str).unwrap_or("status");
    match subcommand {
        "ping" => {
            let reply = agent_request("ping", json!({}), Duration::from_secs(2))?;
            println!(
                "agent pong after {}ms: {}",
                reply.elapsed_ms,
                serde_json::to_string(reply.response.get("result").unwrap_or(&Value::Null))?
            );
        }
        "status" => {
            let reply = agent_request("status", json!({}), Duration::from_secs(2))?;
            println!(
                "{}",
                serde_json::to_string_pretty(reply.response.get("result").unwrap_or(&Value::Null))?
            );
        }
        "logs" => {
            let json_out = args.iter().any(|arg| arg == "--json");
            let reply = agent_request("logs", json!({}), Duration::from_secs(2))?;
            let result = reply.response.get("result").unwrap_or(&Value::Null);
            if json_out {
                println!("{}", serde_json::to_string_pretty(result)?);
            } else if let Some(lines) = result.get("lines").and_then(Value::as_array) {
                for line in lines.iter().filter_map(Value::as_str) {
                    println!("{line}");
                }
                eprintln!(
                    "agent logs: {} line(s), {} dropped, {}ms",
                    result.get("count").and_then(Value::as_u64).unwrap_or(0),
                    result.get("dropped").and_then(Value::as_u64).unwrap_or(0),
                    reply.elapsed_ms
                );
            } else {
                println!("{}", serde_json::to_string_pretty(result)?);
            }
        }
        "timeline" => {
            let json_out = args.iter().any(|arg| arg == "--json");
            let reply = agent_request("timeline", json!({}), Duration::from_secs(2))?;
            let result = reply.response.get("result").unwrap_or(&Value::Null);
            if json_out {
                println!("{}", serde_json::to_string_pretty(result)?);
            } else if let Some(events) = result.get("events").and_then(Value::as_array) {
                for event in events {
                    let uptime_ms = event.get("uptime_ms").and_then(Value::as_u64).unwrap_or(0);
                    let name = event.get("event").and_then(Value::as_str).unwrap_or("");
                    let detail = event.get("detail").and_then(Value::as_str).unwrap_or("");
                    println!("{uptime_ms}\t{name}\t{detail}");
                }
                eprintln!(
                    "agent timeline: {} event(s), {} dropped, {}ms",
                    result.get("count").and_then(Value::as_u64).unwrap_or(0),
                    result.get("dropped").and_then(Value::as_u64).unwrap_or(0),
                    reply.elapsed_ms
                );
            } else {
                println!("{}", serde_json::to_string_pretty(result)?);
            }
        }
        "diagnostics" => {
            agent_diagnostics(&args[1..])?;
        }
        "deploy-magik-bin" => {
            agent_deploy_magik_bin(&args[1..])?;
        }
        "magik" => {
            agent_magik(&args[1..])?;
        }
        "reboot-wait" => {
            agent_reboot_wait(&args[1..])?;
        }
        "boot-profile" => {
            agent_boot_profile(&args[1..])?;
        }
        "-h" | "--help" => agent_usage(),
        other => return Err(format!("unknown agent subcommand: {other}").into()),
    }
    Ok(())
}

fn agent_usage() {
    println!(
        "usage: scripts/mister agent <ping|status|logs|timeline|diagnostics|deploy-magik-bin|magik|reboot-wait|boot-profile>\n       logs [--json]\n       timeline [--json]\n       diagnostics [--out DIR]\n       deploy-magik-bin LOCAL [REMOTE]\n       magik <status|suspend|resume|restart-launcher>\n       reboot-wait [--timeout SECS] [--raw]\n       boot-profile [samples] [--timeout SECS] [--probe-timeout-ms MS] [--sleep-ms MS] [--raw] [--fail-on-timeout]"
    );
}

fn agent_deploy_magik_bin(args: &[String]) -> Result<()> {
    let local = args
        .first()
        .ok_or("agent deploy-magik-bin needs LOCAL [REMOTE]")?;
    let remote = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("/media/fat/mister-magik/mister-magik-fb");
    let total_t = Instant::now();
    let read_t = Instant::now();
    let bytes = fs::read(local)?;
    let read_ms = read_t.elapsed().as_millis();
    let checksum = fnv64_hex(&bytes);
    let requested_encoding =
        env::var("MISTER_AGENT_DEPLOY_ENCODING").unwrap_or_else(|_| "auto".to_string());
    let min_compress_bytes = env::var("MISTER_AGENT_DEPLOY_COMPRESS_MIN_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(AGENT_DEPLOY_COMPRESS_MIN_BYTES);
    let compress_t = Instant::now();
    let should_try_compress = match requested_encoding.as_str() {
        "auto" => bytes.len() >= min_compress_bytes,
        "lz4-block" => true,
        "raw" => false,
        other => return Err(format!("unsupported MISTER_AGENT_DEPLOY_ENCODING: {other}").into()),
    };
    let compressed = if should_try_compress {
        Some(lz4_flex::block::compress(&bytes))
    } else {
        None
    };
    let compress_ms = compress_t.elapsed().as_millis();
    let compression_decision;
    let (encoding, payload) = match (requested_encoding.as_str(), compressed) {
        ("raw", _) => {
            compression_decision = "forced-raw".to_string();
            ("raw", bytes.clone())
        }
        ("auto", None) => {
            compression_decision = format!("below-min-size:{min_compress_bytes}");
            ("raw", bytes.clone())
        }
        ("auto", Some(compressed)) if compressed.len() < bytes.len() => {
            compression_decision = "smaller".to_string();
            ("lz4-block", compressed)
        }
        ("auto", Some(_)) => {
            compression_decision = "not-smaller".to_string();
            ("raw", bytes.clone())
        }
        ("lz4-block", Some(compressed)) => {
            compression_decision = "forced-lz4-block".to_string();
            ("lz4-block", compressed)
        }
        _ => return Err("invalid deploy compression state".into()),
    };
    let args = json!({
        "remote": remote,
        "size": bytes.len() as u64,
        "payload_size": payload.len() as u64,
        "checksum": checksum,
        "encoding": encoding,
    });
    let reply = agent_stream_request(
        "deploy_magik_bin_stream",
        args,
        &payload,
        Duration::from_secs(120),
    )?;
    let result = reply.response.get("result").unwrap_or(&Value::Null);
    let remote_bytes = verify_agent_deploy_result(result, bytes.len() as u64, remote)?;
    println!(
        "agent_deploy_magik_bin local={} remote={} encoding={} compression_decision={} bytes={} remote_bytes={} payload_bytes={} checksum={} total_ms={} read_ms={} compress_ms={} request_ms={} result={}",
        local,
        remote,
        encoding,
        compression_decision,
        bytes.len(),
        remote_bytes,
        payload.len(),
        checksum,
        total_t.elapsed().as_millis(),
        read_ms,
        compress_ms,
        reply.elapsed_ms,
        serde_json::to_string(result)?
    );
    Ok(())
}

fn agent_magik(args: &[String]) -> Result<()> {
    let action = args.first().map(String::as_str).unwrap_or("status");
    match action {
        "status" | "suspend" | "resume" | "restart-launcher" => {}
        "-h" | "--help" => {
            println!("usage: scripts/mister agent magik <status|suspend|resume|restart-launcher>");
            return Ok(());
        }
        other => return Err(format!("unknown agent magik action: {other}").into()),
    }
    let reply = agent_request("magik", json!({"action": action}), Duration::from_secs(5))?;
    let result = reply.response.get("result").unwrap_or(&Value::Null);
    if action == "status" {
        println!("{}", serde_json::to_string_pretty(result)?);
    } else {
        println!(
            "agent magik {action} ok after {}ms: {}",
            reply.elapsed_ms,
            serde_json::to_string(result)?
        );
    }
    Ok(())
}

fn agent_reboot_wait(args: &[String]) -> Result<()> {
    let raw = reboot_raw_from_args(args)?;
    let timeout_secs = option_value(args, "--timeout")
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| {
            args.iter()
                .find(|arg| !arg.starts_with('-'))
                .and_then(|arg| arg.parse::<f64>().ok())
        })
        .unwrap_or(40.0);
    let mode = if raw { "raw" } else { "supervised" };
    let issue_t = Instant::now();
    let reply = agent_request("reboot", json!({"mode": mode}), Duration::from_secs(2))?;
    let issue_ms = issue_t.elapsed().as_millis();
    println!(
        "agent reboot issued to {} after {issue_ms}ms: {}",
        host(),
        serde_json::to_string(reply.response.get("result").unwrap_or(&Value::Null))?
    );

    let start = Instant::now();
    let mut down_ms = None;
    while start.elapsed().as_secs_f64() < 40.0 {
        let ssh_label = tcp_probe_label(Duration::from_millis(100));
        let agent_label = tcp_probe_label_port(AGENT_PORT, Duration::from_millis(100));
        if ssh_label != "ok" && agent_label != "ok" {
            down_ms = Some(start.elapsed().as_millis());
            println!("  device went down after {}ms", opt_ms(down_ms));
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    let mut agent_ready_ms = None;
    let mut ssh_ready_ms = None;
    let mut last_note = String::new();
    while start.elapsed().as_secs_f64() < timeout_secs {
        if agent_ready_ms.is_none() {
            match agent_request("ping", json!({}), Duration::from_millis(300)) {
                Ok(_) => {
                    agent_ready_ms = Some(start.elapsed().as_millis());
                    println!("  agent ready after {}ms", opt_ms(agent_ready_ms));
                }
                Err(err) => last_note = err.to_string(),
            }
        }
        if ssh_ready_ms.is_none() {
            match connect_timed(2) {
                Ok(timed) => {
                    let out = exec(&timed.sess, "cat /proc/uptime", true)?;
                    if out.rc == 0 {
                        ssh_ready_ms = Some(start.elapsed().as_millis());
                        let ssh_uptime = out.stdout.split_whitespace().next().unwrap_or("");
                        println!(
                            "  ssh ready after {}ms; uptime={ssh_uptime}",
                            opt_ms(ssh_ready_ms)
                        );
                    } else {
                        last_note = format!("ssh exec rc {}", out.rc);
                    }
                }
                Err(err) => last_note = err.to_string(),
            }
        }
        if agent_ready_ms.is_some() && ssh_ready_ms.is_some() {
            println!(
                "agent reboot-wait ok mode={mode} down_ms={} agent_ready_ms={} ssh_ready_ms={}",
                opt_ms(down_ms),
                opt_ms(agent_ready_ms),
                opt_ms(ssh_ready_ms)
            );
            return Ok(());
        }
        thread::sleep(Duration::from_millis(150));
    }

    Err(format!(
        "agent reboot-wait timeout mode={mode} down_ms={} agent_ready_ms={} ssh_ready_ms={} last={}",
        opt_ms(down_ms),
        opt_ms(agent_ready_ms),
        opt_ms(ssh_ready_ms),
        last_note
    )
    .into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LauncherRestartOptions {
    env_vars: Vec<(String, String)>,
    clear_env: bool,
    timeout_secs: u64,
    remote_env: String,
}

impl Default for LauncherRestartOptions {
    fn default() -> Self {
        Self {
            env_vars: Vec::new(),
            clear_env: false,
            timeout_secs: 20,
            remote_env: DEFAULT_LAUNCHER_ENV_REMOTE.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LauncherReadyStatus {
    main_ms: u128,
    slint_ms: u128,
    launcher_pid: i64,
    slint_pid: i64,
    frames: u64,
    screen: String,
}

fn launcher_restart(sess: &Session, options: &LauncherRestartOptions) -> Result<()> {
    let started = Instant::now();
    let env_t = Instant::now();
    let env_mode = prepare_launcher_env(sess, options)?;
    let env_ms = env_t.elapsed().as_millis();

    let issue_t = Instant::now();
    issue_launcher_restart(sess)?;
    let issue_ms = issue_t.elapsed().as_millis();

    let ready = wait_launcher_ready(sess, started, Duration::from_secs(options.timeout_secs))?;
    println!(
        "launcher restart ok host={} env={} env_ms={} issue_ms={} ready_ms={} main_status_ms={} slint_status_ms={} launcher_pid={} slint_pid={} frames={} screen={}",
        host(),
        env_mode,
        env_ms,
        issue_ms,
        started.elapsed().as_millis(),
        ready.main_ms,
        ready.slint_ms,
        ready.launcher_pid,
        ready.slint_pid,
        ready.frames,
        ready.screen
    );
    Ok(())
}

fn launcher_restart_help_requested(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
}

fn launcher_restart_usage() {
    println!(
        "usage: scripts/mister launcher-restart [--env KEY=VALUE]... [--clear-env] [--timeout SECS] [--remote-env PATH]"
    );
}

fn parse_launcher_restart_args(args: &[String]) -> Result<LauncherRestartOptions> {
    let mut options = LauncherRestartOptions::default();
    let mut idx = 0usize;
    while idx < args.len() {
        match args[idx].as_str() {
            "--env" => {
                idx += 1;
                let item = args
                    .get(idx)
                    .ok_or("launcher-restart --env needs KEY=VALUE")?;
                let (key, value) = parse_launcher_env_pair(item)?;
                options.env_vars.push((key, value));
            }
            "--clear-env" => {
                options.clear_env = true;
            }
            "--timeout" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or("launcher-restart --timeout needs seconds")?;
                options.timeout_secs = value.parse::<u64>().map_err(|_| {
                    format!("launcher-restart --timeout must be an integer: {value}")
                })?;
                if options.timeout_secs == 0 {
                    return Err("launcher-restart --timeout must be positive".into());
                }
            }
            "--remote-env" => {
                idx += 1;
                options.remote_env = args
                    .get(idx)
                    .ok_or("launcher-restart --remote-env needs a path")?
                    .clone();
            }
            "-h" | "--help" => launcher_restart_usage(),
            other => return Err(format!("unknown launcher-restart option: {other}").into()),
        }
        idx += 1;
    }
    if options.clear_env && !options.env_vars.is_empty() {
        return Err("launcher-restart cannot combine --clear-env with --env".into());
    }
    let _ = remote_parent_dir(&options.remote_env)?;
    Ok(options)
}

fn parse_launcher_env_pair(item: &str) -> Result<(String, String)> {
    let (key, value) = item
        .split_once('=')
        .ok_or_else(|| format!("launcher env must be KEY=VALUE: {item}"))?;
    if !is_launcher_env_key(key) {
        return Err(format!("invalid launcher env key: {key}").into());
    }
    Ok((key.to_string(), value.to_string()))
}

fn is_launcher_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(ch) if ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn launcher_env_text(vars: &[(String, String)]) -> String {
    let mut text = String::new();
    for (key, value) in vars {
        text.push_str("export ");
        text.push_str(key);
        text.push('=');
        text.push_str(&shell_export_quote(value));
        text.push('\n');
    }
    text
}

fn shell_export_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn prepare_launcher_env(sess: &Session, options: &LauncherRestartOptions) -> Result<String> {
    if options.clear_env {
        let out = exec(sess, &format!("rm -f {}", sh(&options.remote_env)), true)?;
        if out.rc != 0 {
            return Err(format!("clear launcher env failed: {}", out.stdout.trim()).into());
        }
        return Ok("cleared".to_string());
    }
    if options.env_vars.is_empty() {
        return Ok("unchanged".to_string());
    }
    let parent = remote_parent_dir(&options.remote_env)?;
    let out = exec(sess, &format!("mkdir -p {}", sh(parent)), true)?;
    if out.rc != 0 {
        return Err(format!("create launcher env parent failed: {}", out.stdout.trim()).into());
    }
    put_bytes(
        sess,
        &options.remote_env,
        launcher_env_text(&options.env_vars).as_bytes(),
    )?;
    Ok(format!("written:{}", options.env_vars.len()))
}

fn remote_parent_dir(remote: &str) -> Result<&str> {
    if !remote.starts_with('/') {
        return Err(
            format!("remote path must be absolute and include a directory: {remote}").into(),
        );
    }
    remote
        .rsplit_once('/')
        .map(|(dir, _)| if dir.is_empty() { "/" } else { dir })
        .ok_or_else(|| {
            format!("remote path must be absolute and include a directory: {remote}").into()
        })
}

fn issue_launcher_restart(sess: &Session) -> Result<()> {
    let out = exec(
        sess,
        &format!(
            "rm -f {} {}; if [ -p /dev/MiSTer_cmd ] && pidof MiSTer_MagiK >/dev/null 2>&1; then printf 'mister_magik_restart_launcher\\n' > /dev/MiSTer_cmd; echo restarted; else echo 'launcher restart unavailable: MiSTer_MagiK or /dev/MiSTer_cmd missing' >&2; exit 12; fi",
            sh(MAIN_STATUS_REMOTE),
            sh(SLINT_STATUS_REMOTE)
        ),
        true,
    )?;
    if out.rc == 0 {
        Ok(())
    } else {
        Err(format!("launcher restart command failed: {}", out.stdout.trim()).into())
    }
}

fn wait_launcher_ready(
    sess: &Session,
    started: Instant,
    timeout: Duration,
) -> Result<LauncherReadyStatus> {
    let mut last_state = String::new();
    while started.elapsed() < timeout {
        let elapsed_ms = started.elapsed().as_millis();
        let main = remote_read(sess, MAIN_STATUS_REMOTE)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let slint = remote_read(sess, SLINT_STATUS_REMOTE)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let state = main
            .as_ref()
            .and_then(|value| value.get("launcher_state"))
            .and_then(Value::as_str)
            .unwrap_or("missing");
        last_state = state.to_string();
        if let Some(ready) = launcher_ready_status(elapsed_ms, main.as_ref(), slint.as_ref()) {
            return Ok(ready);
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!(
        "launcher restart timed out after {}ms; last launcher_state={last_state}",
        timeout.as_millis()
    )
    .into())
}

fn launcher_ready_status(
    elapsed_ms: u128,
    main: Option<&Value>,
    slint: Option<&Value>,
) -> Option<LauncherReadyStatus> {
    let main = main?;
    let slint = slint?;
    if main.get("launcher_state").and_then(Value::as_str) != Some("LauncherActive") {
        return None;
    }
    if slint.get("scene").and_then(Value::as_str) != Some("launcher") {
        return None;
    }
    let frames = slint.get("frames").and_then(Value::as_u64).unwrap_or(0);
    if frames == 0 {
        return None;
    }
    Some(LauncherReadyStatus {
        main_ms: elapsed_ms,
        slint_ms: elapsed_ms,
        launcher_pid: main
            .get("launcher_pid")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        slint_pid: slint.get("pid").and_then(Value::as_i64).unwrap_or_default(),
        frames,
        screen: slint
            .get("screen")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

fn agent_diagnostics(args: &[String]) -> Result<()> {
    let out_dir = option_value(args, "--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("build/agent-diagnostics/{}", unix_secs())));
    fs::create_dir_all(&out_dir)?;

    let bundle = match agent_request("diagnostics", json!({}), Duration::from_secs(3)) {
        Ok(reply) => {
            let mut result = reply.response.get("result").cloned().unwrap_or(Value::Null);
            if let Value::Object(ref mut object) = result {
                object.insert("transport".to_string(), Value::String("agent".to_string()));
                object.insert(
                    "request_ms".to_string(),
                    Value::from(reply.elapsed_ms as u64),
                );
            }
            result
        }
        Err(err) => {
            eprintln!("agent diagnostics unavailable over TCP: {err}; falling back to SSH");
            ssh_diagnostics_bundle(err.to_string())?
        }
    };

    write_diagnostics_bundle(&out_dir, &bundle)?;
    println!("diagnostics_dir={}", out_dir.display());
    Ok(())
}

fn ssh_diagnostics_bundle(agent_error: String) -> Result<Value> {
    let sess = connect(10)?;
    let status = collect_status(&sess)?;
    let ps = exec(&sess, "ps w", true)
        .map(|out| out.stdout)
        .unwrap_or_else(|err| format!("error: {err}"));
    Ok(json!({
        "schema": "mister-magik-agent-diagnostics-v1",
        "transport": "ssh-fallback",
        "agent_error": agent_error,
        "status": status,
        "timeline": Value::Null,
        "agent_logs": Value::Null,
        "net": {
            "carrier": remote_read(&sess, "/sys/class/net/eth0/carrier"),
            "operstate": remote_read(&sess, "/sys/class/net/eth0/operstate"),
            "address": remote_read(&sess, "/sys/class/net/eth0/address"),
            "route": remote_read(&sess, "/proc/net/route"),
            "arp": remote_read(&sess, "/proc/net/arp"),
            "dev": remote_read(&sess, "/proc/net/dev"),
        },
        "processes": {
            "ps": ps,
        },
        "files": {
            "slint_status": remote_read(&sess, "/tmp/mister-magik/status.json"),
            "main_status": remote_read(&sess, "/tmp/mister-magik/main-status.json"),
            "events_tail": tail_remote(&sess, "/tmp/mister-magik/events.jsonl", 80).map(|lines| lines.join("\n")),
            "slint_log_tail": tail_remote(&sess, "/tmp/mister-magik-slint.log", 120).map(|lines| lines.join("\n")),
            "main_log_tail": tail_remote(&sess, "/tmp/mister-magik-main.log", 120).map(|lines| lines.join("\n")),
            "agent_tmp_log_tail": tail_remote(&sess, "/tmp/mister-magik-agent.log", 160).map(|lines| lines.join("\n")),
            "agent_persistent_log_tail": tail_remote(&sess, "/media/fat/mister-magik/bootlogs/agent.log", 160).map(|lines| lines.join("\n")),
            "boot_analytics_tail": tail_remote(&sess, "/tmp/mister-magik-boot-analytics.tsv", 80).map(|lines| lines.join("\n")),
        },
        "crashes": ssh_crash_reports_json(&sess),
    }))
}

fn write_diagnostics_bundle(out_dir: &Path, bundle: &Value) -> Result<()> {
    fs::write(
        out_dir.join("bundle.json"),
        serde_json::to_vec_pretty(bundle)?,
    )?;
    write_json_member(out_dir, "status.json", bundle.get("status"))?;
    write_json_member(out_dir, "timeline.json", bundle.get("timeline"))?;
    write_json_member(out_dir, "agent-logs.json", bundle.get("agent_logs"))?;
    write_json_member(out_dir, "net.json", bundle.get("net"))?;
    write_json_member(out_dir, "processes.json", bundle.get("processes"))?;
    write_json_member(out_dir, "crashes.json", bundle.get("crashes"))?;
    write_json_member(
        out_dir,
        "crash-latest.json",
        bundle.pointer("/crashes/latest"),
    )?;

    write_string_pointer(out_dir, "ps.txt", bundle.pointer("/processes/ps"))?;
    write_string_pointer(
        out_dir,
        "slint-status.json",
        bundle.pointer("/files/slint_status"),
    )?;
    write_string_pointer(
        out_dir,
        "main-status.json",
        bundle.pointer("/files/main_status"),
    )?;
    write_string_pointer(
        out_dir,
        "events-tail.jsonl",
        bundle.pointer("/files/events_tail"),
    )?;
    write_string_pointer(
        out_dir,
        "slint-log-tail.log",
        bundle.pointer("/files/slint_log_tail"),
    )?;
    write_string_pointer(
        out_dir,
        "main-log-tail.log",
        bundle.pointer("/files/main_log_tail"),
    )?;
    write_string_pointer(
        out_dir,
        "agent-tmp-log-tail.log",
        bundle.pointer("/files/agent_tmp_log_tail"),
    )?;
    write_string_pointer(
        out_dir,
        "agent-persistent-log-tail.log",
        bundle.pointer("/files/agent_persistent_log_tail"),
    )?;
    write_string_pointer(
        out_dir,
        "boot-analytics-tail.tsv",
        bundle.pointer("/files/boot_analytics_tail"),
    )?;
    Ok(())
}

fn ssh_crash_reports_json(sess: &Session) -> Value {
    let latest_path = "/media/fat/mister-magik/crashes/latest.json";
    let latest = remote_read(sess, latest_path)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let latest_report_id = latest
        .get("report_id")
        .and_then(Value::as_str)
        .map(|report_id| format!("{report_id}.json"));
    let recent = remote_crash_report_paths(sess, 5, latest_report_id.as_deref())
        .into_iter()
        .map(|path| {
            let report = remote_read(sess, &path)
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or(Value::Null);
            json!({
                "path": path,
                "report": report,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "dir": "/media/fat/mister-magik/crashes",
        "latest_path": latest_path,
        "latest": latest,
        "recent": recent,
    })
}

fn remote_crash_report_paths(
    sess: &Session,
    limit: usize,
    latest_name: Option<&str>,
) -> Vec<String> {
    let cmd = format!(
        "ls -1 {} 2>/dev/null | grep '^report-.*\\.json$' | sort | tail -n {}",
        sh("/media/fat/mister-magik/crashes"),
        limit
    );
    let Ok(out) = exec(sess, &cmd, true) else {
        return Vec::new();
    };
    if out.rc != 0 {
        return Vec::new();
    }
    let mut paths = Vec::new();
    if let Some(name) = latest_name {
        paths.push(format!("/media/fat/mister-magik/crashes/{name}"));
    }
    paths.extend(
        out.stdout
            .lines()
            .filter(|line| Some(*line) != latest_name)
            .map(|line| format!("/media/fat/mister-magik/crashes/{line}")),
    );
    paths.truncate(limit);
    paths
}

fn write_json_member(out_dir: &Path, name: &str, value: Option<&Value>) -> Result<()> {
    if let Some(value) = value {
        if !value.is_null() {
            fs::write(out_dir.join(name), serde_json::to_vec_pretty(value)?)?;
        }
    }
    Ok(())
}

fn write_string_pointer(out_dir: &Path, name: &str, value: Option<&Value>) -> Result<()> {
    if let Some(text) = value.and_then(Value::as_str) {
        fs::write(out_dir.join(name), text)?;
    }
    Ok(())
}

fn agent_token() -> Result<String> {
    if let Ok(token) = env::var("MISTER_AGENT_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    match fs::read_to_string(AGENT_TOKEN_LOCAL) {
        Ok(token) => Ok(token.trim().to_string()),
        Err(err) => {
            eprintln!(
                "warning: agent token unavailable ({AGENT_TOKEN_LOCAL}: {err}); using unauthenticated agent request"
            );
            Ok(String::new())
        }
    }
}

fn agent_request(cmd: &str, args: Value, timeout: Duration) -> Result<AgentResponse> {
    let token = agent_token()?;
    let addr = format!("{}:{AGENT_PORT}", host())
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer agent host")?;
    let request = json!({
        "token": token,
        "id": 1,
        "cmd": cmd,
        "args": args,
    });
    let start = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    writeln!(stream, "{request}")?;
    stream.flush()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    parse_agent_response_line(line, start)
}

fn agent_stream_request(
    cmd: &str,
    args: Value,
    payload: &[u8],
    timeout: Duration,
) -> Result<AgentResponse> {
    let token = agent_token()?;
    let addr = format!("{}:{AGENT_PORT}", host())
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer agent host")?;
    let request = json!({
        "token": token,
        "id": 1,
        "cmd": cmd,
        "args": args,
    });
    let start = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    writeln!(stream, "{request}")?;
    stream.write_all(payload)?;
    stream.flush()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    parse_agent_response_line(line, start)
}

fn parse_agent_response_line(line: String, start: Instant) -> Result<AgentResponse> {
    if line.trim().is_empty() {
        return Err("empty response from agent".into());
    }
    let response: Value = serde_json::from_str(line.trim())?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(AgentResponse {
            response,
            elapsed_ms: start.elapsed().as_millis(),
        })
    } else {
        let error = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("agent command failed");
        Err(error.to_string().into())
    }
}

fn agent_probe_label(timeout: Duration) -> String {
    match agent_request("ping", json!({}), timeout) {
        Ok(_) => "ok".to_string(),
        Err(err) => {
            let text = err.to_string();
            if text.contains("Connection refused") || text.contains("connection refused") {
                "refused".to_string()
            } else if text.contains("timed out") || text.contains("TimedOut") {
                "timeout".to_string()
            } else if text.contains("No route to host") {
                "noroute".to_string()
            } else if text.contains("Host is down") {
                "hostdown".to_string()
            } else {
                text.replace('\t', " ").replace(' ', "_")
            }
        }
    }
}

fn agent_boot_profile(args: &[String]) -> Result<()> {
    let _ = agent_token()?;
    let samples = parse_profile_count(args, 1);
    let raw = reboot_raw_from_args(args)?;
    let fail_on_timeout = args.iter().any(|arg| arg == "--fail-on-timeout");
    let timeout_secs = option_value(args, "--timeout")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(40.0);
    let probe_timeout_ms = option_value(args, "--probe-timeout-ms")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(100);
    let sleep_ms = option_value(args, "--sleep-ms")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(50);
    let mode = if raw { "raw" } else { "supervised" };
    let out_path = "history/toolchain-bench/results-agent.tsv";
    let header = "kind\tts_unix_ms\tsample\tmode\thost\treboot_issue_ms\tdown_ms\tagent_ready_ms\tssh_exec_ready_ms\tagent_first_hostdown_ms\tagent_first_noroute_ms\tagent_first_timeout_ms\tagent_first_refused_ms\tagent_first_other_ms\tagent_ok_count\tagent_hostdown_count\tagent_noroute_count\tagent_timeout_count\tagent_refused_count\tagent_other_count\tresolve_ms\ttcp_ms\thandshake_ms\tauth_ms\texec_ms\tagent_uptime_ms\tssh_uptime\tagent_transitions\tnote";
    println!("{header}");

    let mut recovered = 0usize;
    let mut worst_agent_ready_ms: Option<u128> = None;
    let mut worst_ssh_ready_ms: Option<u128> = None;
    let mut total_noroute = 0u64;
    let mut total_timeout = 0u64;
    let mut total_refused = 0u64;

    for sample in 1..=samples {
        let ts = unix_ms_now();
        let issue_t = Instant::now();
        let reboot_note = {
            let sess = connect(10)?;
            issue_reboot(&sess, raw)?
        };
        let reboot_issue_ms = issue_t.elapsed().as_millis();
        let start = Instant::now();
        let mut down_ms = None;
        while start.elapsed().as_secs_f64() < 40.0 {
            let ssh_label = tcp_probe_label(Duration::from_millis(100));
            let agent_label = tcp_probe_label_port(AGENT_PORT, Duration::from_millis(100));
            if ssh_label != "ok" && agent_label != "ok" {
                down_ms = Some(start.elapsed().as_millis());
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let mut agent_stats = TcpProbeStats::default();
        let mut agent_ready_ms = None;
        let mut agent_uptime_ms = String::new();
        let mut ssh_ready_ms = None;
        let mut resolve_ms = None;
        let mut tcp_ms = None;
        let mut handshake_ms = None;
        let mut auth_ms = None;
        let mut exec_ms = None;
        let mut ssh_uptime = String::new();
        let mut main_status_ms = None;
        let mut launcher_state = String::new();
        let mut note = reboot_note;

        while start.elapsed().as_secs_f64() < timeout_secs {
            let elapsed_ms = start.elapsed().as_millis();
            if agent_ready_ms.is_none() {
                let label = agent_probe_label(Duration::from_millis(probe_timeout_ms));
                agent_stats.observe(&label, elapsed_ms);
                if label == "ok" {
                    agent_ready_ms = Some(elapsed_ms);
                    if let Ok(reply) =
                        agent_request("status", json!({}), Duration::from_millis(500))
                    {
                        agent_uptime_ms = reply
                            .response
                            .pointer("/result/agent/uptime_ms")
                            .and_then(Value::as_u64)
                            .map(|n| n.to_string())
                            .unwrap_or_default();
                    }
                }
            }

            if ssh_ready_ms.is_none() {
                match connect_timed(2) {
                    Ok(timed) => {
                        let exec_t = Instant::now();
                        let out = exec(&timed.sess, "cat /proc/uptime", true)?;
                        let this_exec_ms = exec_t.elapsed().as_millis();
                        if out.rc == 0 {
                            ssh_ready_ms = Some(start.elapsed().as_millis());
                            resolve_ms = Some(timed.resolve_ms);
                            tcp_ms = Some(timed.tcp_ms);
                            handshake_ms = Some(timed.handshake_ms);
                            auth_ms = Some(timed.auth_ms);
                            exec_ms = Some(this_exec_ms);
                            ssh_uptime = out
                                .stdout
                                .split_whitespace()
                                .next()
                                .unwrap_or("")
                                .to_string();

                            let status_deadline = Instant::now() + Duration::from_secs(20);
                            while Instant::now() < status_deadline
                                && start.elapsed().as_secs_f64() < timeout_secs
                            {
                                if let Some(text) =
                                    remote_read(&timed.sess, "/tmp/mister-magik/main-status.json")
                                {
                                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                        main_status_ms = Some(start.elapsed().as_millis());
                                        launcher_state = value
                                            .get("launcher_state")
                                            .and_then(Value::as_str)
                                            .unwrap_or("")
                                            .to_string();
                                        if launcher_state == "LauncherActive" {
                                            break;
                                        }
                                    }
                                }
                                thread::sleep(Duration::from_millis(250));
                            }
                        } else {
                            note = format!("exec rc {}", out.rc);
                        }
                    }
                    Err(err) => {
                        note = err.to_string();
                    }
                }
            }

            if agent_ready_ms.is_some()
                && ssh_ready_ms.is_some()
                && launcher_state == "LauncherActive"
            {
                break;
            }
            thread::sleep(Duration::from_millis(sleep_ms));
        }

        let transitions = agent_stats.transitions.join(",");
        let note = format!(
            "{} main_status_ms={} launcher_state={}",
            note,
            opt_ms(main_status_ms),
            if launcher_state.is_empty() {
                "missing"
            } else {
                &launcher_state
            }
        );
        let row = format!(
            "agent-boot\t{ts}\t{sample}\t{mode}\t{}\t{reboot_issue_ms}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{agent_uptime_ms}\t{ssh_uptime}\t{}\t{}",
            host(),
            opt_ms(down_ms),
            opt_ms(agent_ready_ms),
            opt_ms(ssh_ready_ms),
            opt_ms(agent_stats.first_hostdown_ms),
            opt_ms(agent_stats.first_noroute_ms),
            opt_ms(agent_stats.first_timeout_ms),
            opt_ms(agent_stats.first_refused_ms),
            opt_ms(agent_stats.first_other_ms),
            agent_stats.ok_count,
            agent_stats.hostdown_count,
            agent_stats.noroute_count,
            agent_stats.timeout_count,
            agent_stats.refused_count,
            agent_stats.other_count,
            opt_ms(resolve_ms),
            opt_ms(tcp_ms),
            opt_ms(handshake_ms),
            opt_ms(auth_ms),
            opt_ms(exec_ms),
            transitions.replace('\t', " "),
            note.replace('\t', " ")
        );
        println!("{row}");
        append_profile_row(out_path, header, &row)?;

        total_noroute += agent_stats.noroute_count;
        total_timeout += agent_stats.timeout_count;
        total_refused += agent_stats.refused_count;
        if let Some(ms) = agent_ready_ms {
            worst_agent_ready_ms = Some(worst_agent_ready_ms.map_or(ms, |old| old.max(ms)));
        }
        if let Some(ms) = ssh_ready_ms {
            worst_ssh_ready_ms = Some(worst_ssh_ready_ms.map_or(ms, |old| old.max(ms)));
        }

        let sample_recovered = down_ms.is_some()
            && agent_ready_ms.is_some()
            && ssh_ready_ms.is_some()
            && launcher_state == "LauncherActive";
        if sample_recovered {
            recovered += 1;
        } else if fail_on_timeout {
            return Err(format!(
                "agent boot-profile sample {sample}/{samples} failed mode={mode}: down_ms={} agent_ready_ms={} ssh_exec_ready_ms={} main_status_ms={} launcher_state={} note={}",
                opt_ms(down_ms),
                opt_ms(agent_ready_ms),
                opt_ms(ssh_ready_ms),
                opt_ms(main_status_ms),
                if launcher_state.is_empty() { "missing" } else { &launcher_state },
                note
            )
            .into());
        }
        thread::sleep(Duration::from_secs(2));
    }

    eprintln!(
        "agent boot-profile: {recovered}/{samples} {mode} reboots recovered; worst_agent_ready_ms={} worst_ssh_ready_ms={} noroute={} timeout={} refused={}",
        opt_ms(worst_agent_ready_ms),
        opt_ms(worst_ssh_ready_ms),
        total_noroute,
        total_timeout,
        total_refused
    );
    eprintln!("agent boot-profile: appended {samples} row(s) to {out_path}");
    Ok(())
}

fn boot_net_profile(args: &[String]) -> Result<()> {
    let samples = parse_profile_count(args, 3);
    let raw = reboot_raw_from_args(args)?;
    let timeout_secs = option_value(args, "--timeout")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(120.0);
    let mode = if raw { "raw" } else { "supervised" };
    let out_path = "history/toolchain-bench/results-boot-net.tsv";
    let header = "kind\tts_unix_ms\tsample\tmode\thost\treboot_issue_ms\tdown_ms\ttcp22_ms\tssh_exec_ready_ms\tresolve_ms\ttcp_ms\thandshake_ms\tauth_ms\texec_ms\tmain_status_ms\tslint_status_ms\tuptime\tlauncher_state\tslint_frames\tnote";
    println!("{header}");
    for sample in 1..=samples {
        let ts = unix_ms_now();
        let issue_t = Instant::now();
        let reboot_note = {
            let sess = connect(10)?;
            issue_reboot(&sess, raw)?
        };
        let reboot_issue_ms = issue_t.elapsed().as_millis();
        let start = Instant::now();
        let mut down_ms = None;
        while start.elapsed().as_secs_f64() < 40.0 {
            if !port_open(Duration::from_millis(200)) {
                down_ms = Some(start.elapsed().as_millis());
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let mut tcp22_ms = None;
        let mut ssh_ready_ms = None;
        let mut resolve_ms = None;
        let mut tcp_ms = None;
        let mut handshake_ms = None;
        let mut auth_ms = None;
        let mut exec_ms = None;
        let mut uptime = String::new();
        let mut launcher_state = String::new();
        let mut slint_frames = String::new();
        let mut main_status_ms = None;
        let mut slint_status_ms = None;
        let mut note = reboot_note;

        while start.elapsed().as_secs_f64() < timeout_secs {
            if tcp22_ms.is_none() && port_open(Duration::from_millis(150)) {
                tcp22_ms = Some(start.elapsed().as_millis());
            }
            match connect_timed(2) {
                Ok(timed) => {
                    let exec_t = Instant::now();
                    let out = exec(&timed.sess, "cat /proc/uptime", true)?;
                    let this_exec_ms = exec_t.elapsed().as_millis();
                    if out.rc == 0 {
                        ssh_ready_ms = Some(start.elapsed().as_millis());
                        resolve_ms = Some(timed.resolve_ms);
                        tcp_ms = Some(timed.tcp_ms);
                        handshake_ms = Some(timed.handshake_ms);
                        auth_ms = Some(timed.auth_ms);
                        exec_ms = Some(this_exec_ms);
                        uptime = out
                            .stdout
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string();

                        let status_deadline = Instant::now() + Duration::from_secs(20);
                        while Instant::now() < status_deadline {
                            if main_status_ms.is_none() {
                                if let Some(text) =
                                    remote_read(&timed.sess, "/tmp/mister-magik/main-status.json")
                                {
                                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                        main_status_ms = Some(start.elapsed().as_millis());
                                        launcher_state = value
                                            .get("launcher_state")
                                            .and_then(Value::as_str)
                                            .unwrap_or("")
                                            .to_string();
                                    }
                                }
                            }
                            if slint_status_ms.is_none() {
                                if let Some(text) =
                                    remote_read(&timed.sess, "/tmp/mister-magik/status.json")
                                {
                                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                        slint_status_ms = Some(start.elapsed().as_millis());
                                        slint_frames = value
                                            .get("frames")
                                            .and_then(Value::as_u64)
                                            .map(|n| n.to_string())
                                            .unwrap_or_default();
                                    }
                                }
                            }
                            if main_status_ms.is_some() && slint_status_ms.is_some() {
                                break;
                            }
                            thread::sleep(Duration::from_millis(250));
                        }
                        break;
                    }
                    note = format!("exec rc {}", out.rc);
                }
                Err(err) => {
                    note = err.to_string();
                }
            }
            thread::sleep(Duration::from_millis(250));
        }

        let row = format!(
            "boot-net\t{ts}\t{sample}\t{mode}\t{}\t{reboot_issue_ms}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{uptime}\t{launcher_state}\t{slint_frames}\t{}",
            host(),
            opt_ms(down_ms),
            opt_ms(tcp22_ms),
            opt_ms(ssh_ready_ms),
            opt_ms(resolve_ms),
            opt_ms(tcp_ms),
            opt_ms(handshake_ms),
            opt_ms(auth_ms),
            opt_ms(exec_ms),
            opt_ms(main_status_ms),
            opt_ms(slint_status_ms),
            note.replace('\t', " ")
        );
        println!("{row}");
        append_profile_row(out_path, header, &row)?;
        thread::sleep(Duration::from_secs(2));
    }
    eprintln!("boot-net-profile: appended {samples} row(s) to {out_path}");
    Ok(())
}

#[derive(Default)]
struct TcpProbeStats {
    ok_count: u64,
    hostdown_count: u64,
    noroute_count: u64,
    timeout_count: u64,
    refused_count: u64,
    other_count: u64,
    first_ok_ms: Option<u128>,
    first_hostdown_ms: Option<u128>,
    first_noroute_ms: Option<u128>,
    first_timeout_ms: Option<u128>,
    first_refused_ms: Option<u128>,
    first_other_ms: Option<u128>,
    last_label: String,
    transitions: Vec<String>,
}

impl TcpProbeStats {
    fn observe(&mut self, label: &str, elapsed_ms: u128) {
        match label {
            "ok" => {
                self.ok_count += 1;
                self.first_ok_ms.get_or_insert(elapsed_ms);
            }
            "hostdown" => {
                self.hostdown_count += 1;
                self.first_hostdown_ms.get_or_insert(elapsed_ms);
            }
            "noroute" => {
                self.noroute_count += 1;
                self.first_noroute_ms.get_or_insert(elapsed_ms);
            }
            "timeout" => {
                self.timeout_count += 1;
                self.first_timeout_ms.get_or_insert(elapsed_ms);
            }
            "refused" => {
                self.refused_count += 1;
                self.first_refused_ms.get_or_insert(elapsed_ms);
            }
            _ => {
                self.other_count += 1;
                self.first_other_ms.get_or_insert(elapsed_ms);
            }
        }

        if self.last_label != label {
            self.transitions.push(format!("{elapsed_ms}:{label}"));
            self.last_label = label.to_string();
        }
    }
}

fn tcp_probe_label(timeout: Duration) -> String {
    tcp_probe_label_port(22, timeout)
}

fn tcp_probe_label_port(port: u16, timeout: Duration) -> String {
    let addr = match format!("{}:{port}", host()).to_socket_addrs() {
        Ok(mut addrs) => match addrs.next() {
            Some(addr) => addr,
            None => return "resolve_none".to_string(),
        },
        Err(err) => return format!("resolve_{}", err.kind() as u8),
    };

    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => "ok".to_string(),
        Err(err) => match err.raw_os_error() {
            Some(64) => "hostdown".to_string(),
            Some(65) => "noroute".to_string(),
            Some(60) => "timeout".to_string(),
            Some(61) => "refused".to_string(),
            Some(code) => format!("os{code}"),
            None if err.kind() == io::ErrorKind::TimedOut => "timeout".to_string(),
            None if err.kind() == io::ErrorKind::ConnectionRefused => "refused".to_string(),
            None => format!("{:?}", err.kind()).to_lowercase(),
        },
    }
}

fn host_wait_diagnostics() -> String {
    let host = host();
    let tcp = tcp_probe_label(Duration::from_millis(500));
    let arp = command_summary("arp", &["-an"], Some(&host));
    let ping = if cfg!(target_os = "macos") {
        command_summary("ping", &["-c", "1", "-W", "1000", &host], None)
    } else {
        command_summary("ping", &["-c", "1", "-W", "1", &host], None)
    };
    format!("tcp={tcp}; arp={arp}; ping={ping}")
}

fn command_summary(program: &str, args: &[&str], contains: Option<&str>) -> String {
    match Command::new(program).args(args).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut lines = stdout
                .lines()
                .chain(stderr.lines())
                .filter(|line| contains.map(|needle| line.contains(needle)).unwrap_or(true))
                .map(str::trim)
                .filter(|line| !line.is_empty());
            let text = lines.next().unwrap_or("no matching output");
            format!(
                "rc={} {}",
                output.status.code().unwrap_or(-1),
                text.replace('\t', " ")
            )
        }
        Err(err) => format!("error={}", err),
    }
}

fn boot_tcp_profile(args: &[String]) -> Result<()> {
    let samples = parse_profile_count(args, 1);
    let raw = reboot_raw_from_args(args)?;
    let timeout_secs = option_value(args, "--timeout")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(40.0);
    let probe_timeout_ms = option_value(args, "--probe-timeout-ms")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(100);
    let sleep_ms = option_value(args, "--sleep-ms")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(50);
    let mode = if raw { "raw" } else { "supervised" };
    let out_path = "history/toolchain-bench/results-boot-tcp.tsv";
    let header = "kind\tts_unix_ms\tsample\tmode\thost\treboot_issue_ms\tdown_ms\tfirst_ok_ms\tssh_exec_ready_ms\tfirst_hostdown_ms\tfirst_noroute_ms\tfirst_timeout_ms\tfirst_refused_ms\tfirst_other_ms\tok_count\thostdown_count\tnoroute_count\ttimeout_count\trefused_count\tother_count\tresolve_ms\ttcp_ms\thandshake_ms\tauth_ms\texec_ms\tuptime\ttransitions\tnote";
    println!("{header}");

    for sample in 1..=samples {
        let ts = unix_ms_now();
        let issue_t = Instant::now();
        let reboot_note = {
            let sess = connect(10)?;
            issue_reboot(&sess, raw)?
        };
        let reboot_issue_ms = issue_t.elapsed().as_millis();
        let start = Instant::now();
        let mut down_ms = None;
        while start.elapsed().as_secs_f64() < 40.0 {
            if !port_open(Duration::from_millis(200)) {
                down_ms = Some(start.elapsed().as_millis());
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let mut stats = TcpProbeStats::default();
        let mut ssh_ready_ms = None;
        let mut resolve_ms = None;
        let mut tcp_ms = None;
        let mut handshake_ms = None;
        let mut auth_ms = None;
        let mut exec_ms = None;
        let mut uptime = String::new();
        let mut note = reboot_note;

        while start.elapsed().as_secs_f64() < timeout_secs {
            let elapsed_ms = start.elapsed().as_millis();
            let label = tcp_probe_label(Duration::from_millis(probe_timeout_ms));
            stats.observe(&label, elapsed_ms);
            if label == "ok" {
                break;
            }
            thread::sleep(Duration::from_millis(sleep_ms));
        }

        let ssh_deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < ssh_deadline {
            match connect_timed(2) {
                Ok(timed) => {
                    let exec_t = Instant::now();
                    let out = exec(&timed.sess, "cat /proc/uptime", true)?;
                    let this_exec_ms = exec_t.elapsed().as_millis();
                    if out.rc == 0 {
                        ssh_ready_ms = Some(start.elapsed().as_millis());
                        resolve_ms = Some(timed.resolve_ms);
                        tcp_ms = Some(timed.tcp_ms);
                        handshake_ms = Some(timed.handshake_ms);
                        auth_ms = Some(timed.auth_ms);
                        exec_ms = Some(this_exec_ms);
                        uptime = out
                            .stdout
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string();
                        break;
                    }
                    note = format!("exec rc {}", out.rc);
                }
                Err(err) => {
                    note = err.to_string();
                }
            }
            thread::sleep(Duration::from_millis(150));
        }

        let transitions = stats.transitions.join(",");
        let row = format!(
            "boot-tcp\t{ts}\t{sample}\t{mode}\t{}\t{reboot_issue_ms}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{uptime}\t{}\t{}",
            host(),
            opt_ms(down_ms),
            opt_ms(stats.first_ok_ms),
            opt_ms(ssh_ready_ms),
            opt_ms(stats.first_hostdown_ms),
            opt_ms(stats.first_noroute_ms),
            opt_ms(stats.first_timeout_ms),
            opt_ms(stats.first_refused_ms),
            opt_ms(stats.first_other_ms),
            stats.ok_count,
            stats.hostdown_count,
            stats.noroute_count,
            stats.timeout_count,
            stats.refused_count,
            stats.other_count,
            opt_ms(resolve_ms),
            opt_ms(tcp_ms),
            opt_ms(handshake_ms),
            opt_ms(auth_ms),
            opt_ms(exec_ms),
            transitions.replace('\t', " "),
            note.replace('\t', " ")
        );
        println!("{row}");
        append_profile_row(out_path, header, &row)?;
        thread::sleep(Duration::from_secs(2));
    }

    eprintln!("boot-tcp-profile: appended {samples} row(s) to {out_path}");
    Ok(())
}

fn watch_external_reboot(args: &[String]) -> Result<()> {
    let timeout_secs = option_value(args, "--timeout")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(120.0);
    let wait_down_secs = option_value(args, "--wait-down")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(180.0);
    let out_path = "history/toolchain-bench/results-boot-net.tsv";
    let header = "kind\tts_unix_ms\tsample\tmode\thost\treboot_issue_ms\tdown_ms\ttcp22_ms\tssh_exec_ready_ms\tresolve_ms\ttcp_ms\thandshake_ms\tauth_ms\texec_ms\tmain_status_ms\tslint_status_ms\tuptime\tlauncher_state\tslint_frames\tnote";
    println!("{header}");
    eprintln!(
        "watch-reboot: waiting up to {wait_down_secs:.0}s for {}:22 to go down...",
        host()
    );
    let ts = unix_ms_now();
    let wait_start = Instant::now();
    while wait_start.elapsed().as_secs_f64() < wait_down_secs {
        if !port_open(Duration::from_millis(200)) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if wait_start.elapsed().as_secs_f64() >= wait_down_secs {
        return Err(format!("device did not go down within {wait_down_secs:.0}s").into());
    }
    let start = Instant::now();
    eprintln!("watch-reboot: device went down; timing reconnect...");

    let mut tcp22_ms = None;
    let mut ssh_ready_ms = None;
    let mut resolve_ms = None;
    let mut tcp_ms = None;
    let mut handshake_ms = None;
    let mut auth_ms = None;
    let mut exec_ms = None;
    let mut uptime = String::new();
    let mut launcher_state = String::new();
    let mut slint_frames = String::new();
    let mut main_status_ms = None;
    let mut slint_status_ms = None;
    let mut note = String::from("external");

    while start.elapsed().as_secs_f64() < timeout_secs {
        if tcp22_ms.is_none() && port_open(Duration::from_millis(150)) {
            tcp22_ms = Some(start.elapsed().as_millis());
        }
        match connect_timed(2) {
            Ok(timed) => {
                let exec_t = Instant::now();
                let out = exec(&timed.sess, "cat /proc/uptime", true)?;
                let this_exec_ms = exec_t.elapsed().as_millis();
                if out.rc == 0 {
                    ssh_ready_ms = Some(start.elapsed().as_millis());
                    resolve_ms = Some(timed.resolve_ms);
                    tcp_ms = Some(timed.tcp_ms);
                    handshake_ms = Some(timed.handshake_ms);
                    auth_ms = Some(timed.auth_ms);
                    exec_ms = Some(this_exec_ms);
                    uptime = out
                        .stdout
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string();

                    let status_deadline = Instant::now() + Duration::from_secs(20);
                    while Instant::now() < status_deadline {
                        if main_status_ms.is_none() {
                            if let Some(text) =
                                remote_read(&timed.sess, "/tmp/mister-magik/main-status.json")
                            {
                                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                    main_status_ms = Some(start.elapsed().as_millis());
                                    launcher_state = value
                                        .get("launcher_state")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string();
                                }
                            }
                        }
                        if slint_status_ms.is_none() {
                            if let Some(text) =
                                remote_read(&timed.sess, "/tmp/mister-magik/status.json")
                            {
                                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                    slint_status_ms = Some(start.elapsed().as_millis());
                                    slint_frames = value
                                        .get("frames")
                                        .and_then(Value::as_u64)
                                        .map(|n| n.to_string())
                                        .unwrap_or_default();
                                }
                            }
                        }
                        if main_status_ms.is_some() && slint_status_ms.is_some() {
                            break;
                        }
                        thread::sleep(Duration::from_millis(250));
                    }
                    break;
                }
                note = format!("exec rc {}", out.rc);
            }
            Err(err) => {
                note = err.to_string();
            }
        }
        thread::sleep(Duration::from_millis(250));
    }

    let row = format!(
        "boot-net\t{ts}\t1\texternal\t{}\t\t0\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{uptime}\t{launcher_state}\t{slint_frames}\t{}",
        host(),
        opt_ms(tcp22_ms),
        opt_ms(ssh_ready_ms),
        opt_ms(resolve_ms),
        opt_ms(tcp_ms),
        opt_ms(handshake_ms),
        opt_ms(auth_ms),
        opt_ms(exec_ms),
        opt_ms(main_status_ms),
        opt_ms(slint_status_ms),
        note.replace('\t', " ")
    );
    println!("{row}");
    append_profile_row(out_path, header, &row)?;
    if ssh_ready_ms.is_some() {
        Ok(())
    } else {
        Err(format!("device not ready after {timeout_secs:.0}s").into())
    }
}

fn opt_ms(value: Option<u128>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

fn run_library_db_query(sess: &Session, args: &[String]) -> Result<()> {
    let query_args = library_db_query_args(args);
    let quoted_args = query_args
        .iter()
        .map(|arg| sh(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let command = format!("/media/fat/mister-magik/mister-magik-fb library-sql {quoted_args}");
    let out = exec(sess, &command, true)?;
    if out.rc != 0 && library_sql_command_unavailable(&out.stdout, &out.stderr) {
        eprintln!(
            "scripts/mister db: remote library-sql unavailable; using SFTP local-query fallback"
        );
        let output = run_library_db_query_via_sftp(sess, &query_args)?;
        print!("{output}");
        if !output.ends_with('\n') {
            println!();
        }
        return Ok(());
    }
    print!("{}", out.stdout);
    if !out.stderr.trim().is_empty() {
        eprint!("[stderr] {}", out.stderr);
    }
    std::process::exit(out.rc);
}

fn library_db_query_args(args: &[String]) -> Vec<String> {
    if args.is_empty() {
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
    }
}

fn library_sql_command_unavailable(stdout: &str, stderr: &str) -> bool {
    let text = format!("{stdout}\n{stderr}");
    text.contains("unknown command 'library-sql'")
}

fn run_library_db_query_via_sftp(sess: &Session, args: &[String]) -> Result<String> {
    let (remote_path, query) = parse_library_db_query(args)?;
    let local_path = temporary_library_db_path();
    get(sess, &remote_path, &local_path)?;
    let result = run_local_read_only_sqlite_query(&local_path, &query);
    let _ = fs::remove_file(&local_path);
    result
}

fn parse_library_db_query(args: &[String]) -> Result<(String, String)> {
    let mut remote_path = DEFAULT_REMOTE_LIBRARY_DB.to_string();
    let mut query_parts = Vec::new();
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--path" => {
                let Some(value) = args.get(i + 1) else {
                    return Err("db: --path needs a value".into());
                };
                remote_path = value.to_string();
                i += 2;
            }
            other => {
                query_parts.push(other.to_string());
                i += 1;
            }
        }
    }
    if query_parts.is_empty() {
        return Err("usage: scripts/mister db [--path PATH] SELECT ...".into());
    }
    let query = query_parts.join(" ");
    if !library_db_query_is_read_only(&query) {
        return Err("scripts/mister db only allows read-only SELECT/WITH queries".into());
    }
    Ok((remote_path, query))
}

fn library_db_query_is_read_only(query: &str) -> bool {
    let tokens = library_db_query_tokens(query);
    let Some(first) = tokens.first().map(String::as_str) else {
        return false;
    };
    (first == "select" || first == "with") && !library_db_query_tokens_contain_write(&tokens)
}

fn library_db_query_tokens_contain_write(tokens: &[String]) -> bool {
    tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "insert"
                | "update"
                | "delete"
                | "replace"
                | "create"
                | "drop"
                | "alter"
                | "pragma"
                | "attach"
                | "detach"
                | "vacuum"
                | "reindex"
        )
    })
}

fn library_db_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut chars = query.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' | '"' => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
                while let Some(quoted) = chars.next() {
                    if quoted == ch {
                        if chars.peek() == Some(&ch) {
                            let _ = chars.next();
                            continue;
                        }
                        break;
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
                let _ = chars.next();
                for comment in chars.by_ref() {
                    if comment == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
                let _ = chars.next();
                let mut prev = '\0';
                for comment in chars.by_ref() {
                    if prev == '*' && comment == '/' {
                        break;
                    }
                    prev = comment;
                }
            }
            ch if ch.is_ascii_alphanumeric() || ch == '_' => token.push(ch.to_ascii_lowercase()),
            _ => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

fn temporary_library_db_path() -> PathBuf {
    let mut path = env::temp_dir();
    path.push(format!(
        "mister-library-db-{}-{}.sqlite3",
        unix_ms_now(),
        std::process::id()
    ));
    path
}

fn run_local_read_only_sqlite_query(path: &Path, query: &str) -> Result<String> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()).into());
    }
    if metadata.len() == 0 {
        return Err(format!("{} is empty", path.display()).into());
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    let mut stmt = conn.prepare(query)?;
    let column_count = stmt.column_count();
    let mut out = String::new();
    if column_count > 0 {
        out.push_str(&stmt.column_names().join("\t"));
        out.push('\n');
    }
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        for col in 0..column_count {
            if col > 0 {
                out.push('\t');
            }
            out.push_str(&sqlite_cell_to_string(row, col)?);
        }
        out.push('\n');
    }
    Ok(out)
}

fn sqlite_cell_to_string(row: &rusqlite::Row<'_>, col: usize) -> Result<String> {
    use rusqlite::types::ValueRef;

    match row.get_ref(col)? {
        ValueRef::Null => Ok(String::new()),
        ValueRef::Integer(value) => Ok(value.to_string()),
        ValueRef::Real(value) => Ok(value.to_string()),
        ValueRef::Text(value) => Ok(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(value) => Ok(format!("<blob:{}>", value.len())),
    }
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

fn userspace_ready_fast() -> Option<String> {
    let timed = connect_timed(2).ok()?;
    let out = exec(&timed.sess, "pidof MiSTer || echo BOOTING", true).ok()?;
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
    let mut last_print = Duration::MAX;
    while start.elapsed().as_secs_f64() < max_seconds {
        attempt += 1;
        let elapsed = start.elapsed().as_secs_f64();
        if port_open(Duration::from_millis(150)) {
            if let Some(status) = userspace_ready_fast() {
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
        if last_print == Duration::MAX || start.elapsed().saturating_sub(last_print).as_secs() >= 1
        {
            println!("  [{elapsed:5.1}s] waiting for ssh...");
            last_print = start.elapsed();
        }
        thread::sleep(Duration::from_millis(250));
    }
    println!("TIMEOUT: device not ready after {max_seconds:.0}s");
    println!("diagnostics: {}", host_wait_diagnostics());
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

fn ensure_stock_inittab(sess: &Session, dry_run: bool) -> Result<()> {
    const INITTAB: &str = "/etc/inittab";
    let input = remote_read(sess, INITTAB).ok_or("could not read /etc/inittab")?;
    let edited = ensure_stock_inittab_text(&input);
    if dry_run {
        print!("{edited}");
        return Ok(());
    }
    let tmp = "/tmp/inittab.mister-tool-new";
    remote_write(sess, tmp, edited.as_bytes())?;
    let out = exec(
        sess,
        &format!(
            "mount -o remount,rw / 2>/dev/null || true; cp {} {}; sync",
            sh(tmp),
            sh(INITTAB)
        ),
        true,
    )?;
    if out.rc != 0 {
        return Err(format!("failed to replace {INITTAB}: {}", out.stdout).into());
    }
    println!("inittab ensured -> stock MiSTer");
    Ok(())
}

fn ensure_stock_inittab_text(input: &str) -> String {
    let newline = if input.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = Vec::new();
    let mut wrote = false;
    for raw in input.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with("::sysinit:/media/fat/MiSTer ") && line.ends_with('&') {
            if !wrote {
                out.push("::sysinit:/media/fat/MiSTer &".to_string());
                wrote = true;
            }
            continue;
        }
        if line.starts_with("::sysinit:/media/fat/MiSTer_MagiK")
            || line.starts_with("::sysinit:/media/fat/mister-magik/boot.sh")
        {
            continue;
        }
        out.push(line.to_string());
    }
    if !wrote {
        out.push("::sysinit:/media/fat/MiSTer &".to_string());
    }
    let mut edited = out.join(newline);
    edited.push_str(newline);
    edited
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

fn raw_to_png_cli(args: &[String]) -> Result<()> {
    if args.len() < 4 {
        return Err(
            "raw-to-png needs <raw> <width> <height> <out.png> [--stride N] [--bpp 16|32]".into(),
        );
    }
    let raw_path = Path::new(&args[0]);
    let width = args[1].parse::<usize>()?;
    let height = args[2].parse::<usize>()?;
    let out_path = Path::new(&args[3]);
    let mut stride = None;
    let mut bpp = 32usize;
    let mut i = 4;
    while i < args.len() {
        match args[i].as_str() {
            "--stride" => {
                let value = args
                    .get(i + 1)
                    .ok_or("raw-to-png --stride needs a byte count")?;
                stride = Some(value.parse::<usize>()?);
                i += 2;
            }
            "--bpp" => {
                let value = args.get(i + 1).ok_or("raw-to-png --bpp needs 16 or 32")?;
                bpp = value.parse::<usize>()?;
                i += 2;
            }
            other => return Err(format!("unknown raw-to-png option: {other}").into()),
        }
    }

    raw_to_png(raw_path, width, height, out_path, stride, bpp)
}

fn raw_to_png(
    raw_path: &Path,
    w: usize,
    h: usize,
    out_path: &Path,
    stride: Option<usize>,
    bpp: usize,
) -> Result<()> {
    let raw = fs::read(raw_path)?;
    let bytes_per_pixel = match bpp {
        16 => 2,
        32 => 4,
        _ => return Err(format!("unsupported raw framebuffer bpp: {bpp}").into()),
    };
    let packed_stride = w
        .checked_mul(bytes_per_pixel)
        .ok_or("raw dimensions overflow")?;
    let stride = stride.unwrap_or(packed_stride);
    if stride < packed_stride {
        return Err(
            format!("raw stride {stride} is smaller than packed row {packed_stride}").into(),
        );
    }
    let geometry = FbGeometry {
        width: w,
        height: h,
        stride,
        bpp,
    };
    let expected = geometry.bytes()?;
    if raw.len() < expected {
        return Err(format!(
            "{} has {} bytes, expected at least {expected}",
            raw_path.display(),
            raw.len()
        )
        .into());
    }
    write_png_bgrx_stride(&raw[..expected], &geometry, out_path)
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
    args.windows(2)
        .find(|pair| pair[0] == name && !looks_like_option_token(&pair[1]))
        .map(|pair| pair[1].clone())
}

fn option_values(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|pair| pair[0] == name)
        .filter(|pair| !looks_like_option_token(&pair[1]))
        .map(|pair| pair[1].clone())
        .collect()
}

fn looks_like_option_token(value: &str) -> bool {
    value.starts_with("--")
        || value
            .strip_prefix('-')
            .and_then(|rest| rest.chars().next())
            .is_some_and(|ch| ch.is_ascii_alphabetic())
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
    fn launcher_restart_args_collect_env_and_timeout() {
        let args = vec![
            "--env".to_string(),
            "MISTER_LAUNCHER_START_SCREEN=arcade".to_string(),
            "--env".to_string(),
            "MISTER_PREVIEW_SCROLL_TRACE=/tmp/trace.tsv".to_string(),
            "--timeout".to_string(),
            "30".to_string(),
        ];

        let options = parse_launcher_restart_args(&args).unwrap();

        assert_eq!(options.timeout_secs, 30);
        assert_eq!(options.remote_env, DEFAULT_LAUNCHER_ENV_REMOTE);
        assert_eq!(
            options.env_vars,
            vec![
                (
                    "MISTER_LAUNCHER_START_SCREEN".to_string(),
                    "arcade".to_string()
                ),
                (
                    "MISTER_PREVIEW_SCROLL_TRACE".to_string(),
                    "/tmp/trace.tsv".to_string()
                )
            ]
        );
    }

    #[test]
    fn launcher_restart_args_reject_bad_env_and_clear_conflict() {
        assert!(
            parse_launcher_restart_args(&["--env".to_string(), "BAD-NAME=value".to_string()])
                .is_err()
        );
        assert!(parse_launcher_restart_args(&[
            "--clear-env".to_string(),
            "--env".to_string(),
            "MISTER_CATALOG_REFRESH=off".to_string()
        ])
        .is_err());
        assert!(parse_launcher_restart_args(&[
            "--clear-env".to_string(),
            "--remote-env".to_string(),
            "relative/launcher.env".to_string()
        ])
        .is_err());
    }

    #[test]
    fn launcher_env_text_shell_quotes_values() {
        let text = launcher_env_text(&[
            ("MISTER_CATALOG_REFRESH".to_string(), "off".to_string()),
            ("MISTER_LABEL".to_string(), "kid's test".to_string()),
        ]);

        assert!(text.contains("export MISTER_CATALOG_REFRESH='off'\n"));
        assert!(text.contains("export MISTER_LABEL='kid'\"'\"'s test'\n"));
    }

    #[test]
    fn library_db_query_allows_comments_before_read_only_queries() {
        let args = vec![
            "--path".to_string(),
            "/tmp/library.sqlite3".to_string(),
            "-- comment\n/* more */ WITH recent AS (SELECT 'delete from games')".to_string(),
            "SELECT * FROM recent".to_string(),
        ];

        let (path, query) = parse_library_db_query(&args).expect("read-only query");

        assert_eq!(path, "/tmp/library.sqlite3");
        assert!(query.contains("WITH recent"));
    }

    #[test]
    fn library_db_query_rejects_with_write_statements() {
        for query in [
            "WITH doomed AS (SELECT 1) DELETE FROM games",
            "WITH changed AS (SELECT 1) UPDATE games SET title='x'",
            "WITH created AS (SELECT 1) INSERT INTO games(title) VALUES('x')",
            "SELECT 1; DELETE FROM games",
            "/* comment */ PRAGMA writable_schema=ON",
        ] {
            let err = parse_library_db_query(&[query.to_string()])
                .expect_err("write-capable query should be rejected");
            assert!(
                err.to_string().contains("read-only SELECT/WITH"),
                "{query}: {err}"
            );
        }
    }

    #[test]
    fn launcher_remote_env_parent_requires_absolute_path() {
        assert_eq!(
            remote_parent_dir("/media/fat/mister-magik/launcher.env").unwrap(),
            "/media/fat/mister-magik"
        );
        assert_eq!(remote_parent_dir("/launcher.env").unwrap(), "/");
        assert!(remote_parent_dir("relative/launcher.env").is_err());
    }

    #[test]
    fn launcher_ready_requires_main_and_new_slint_status() {
        let main = json!({
            "launcher_state": "LauncherActive",
            "launcher_pid": 42
        });
        let slint = json!({
            "scene": "launcher",
            "pid": 43,
            "frames": 2,
            "screen": "arcade"
        });

        let ready = launcher_ready_status(125, Some(&main), Some(&slint)).unwrap();

        assert_eq!(ready.launcher_pid, 42);
        assert_eq!(ready.slint_pid, 43);
        assert_eq!(ready.frames, 2);
        assert_eq!(ready.screen, "arcade");
        assert!(launcher_ready_status(125, Some(&main), None).is_none());
        assert!(launcher_ready_status(
            125,
            Some(&main),
            Some(&json!({"scene": "launcher", "frames": 0}))
        )
        .is_none());
    }

    #[test]
    fn reboot_remote_command_supervised_uses_magik_command() {
        let cmd = reboot_remote_command(false);

        assert!(cmd.contains("mister_magik_reboot"));
        assert!(cmd.contains("/dev/MiSTer_cmd"));
        assert!(cmd.contains("MiSTer_MagiK"));
        assert!(!cmd.contains("/sbin/reboot"));
    }

    #[test]
    fn reboot_remote_command_raw_uses_linux_reboot() {
        let cmd = reboot_remote_command(true);

        assert!(cmd.contains("/sbin/reboot"));
        assert!(!cmd.contains("mister_magik_reboot"));
    }

    #[test]
    fn reboot_defaults_to_supervised_and_raw_flag_is_removed_before_timeout_parse() {
        let mut args = vec!["--raw".to_string(), "180".to_string()];

        assert!(take_reboot_raw_flag(&mut args).unwrap());
        assert_eq!(args, vec!["180"]);
        assert!(!take_reboot_raw_flag(&mut args).unwrap());
    }

    #[test]
    fn reboot_raw_and_supervised_flags_conflict() {
        let mut args = vec!["--raw".to_string(), "--supervised".to_string()];

        assert!(take_reboot_raw_flag(&mut args).is_err());
    }

    #[test]
    fn stock_inittab_mutator_removes_old_magik_entries() {
        let input = "::sysinit:/bin/mount -a\r\n::sysinit:/media/fat/MiSTer_MagiK &\r\n::sysinit:/media/fat/mister-magik/boot.sh &\r\n";

        let edited = ensure_stock_inittab_text(input);

        assert!(edited.contains("::sysinit:/bin/mount -a\r\n"));
        assert!(edited.contains("::sysinit:/media/fat/MiSTer &\r\n"));
        assert!(!edited.contains("MiSTer_MagiK"));
        assert!(!edited.contains("mister-magik/boot.sh"));
    }

    #[test]
    fn stock_inittab_mutator_deduplicates_stock_entry() {
        let input =
            "::sysinit:/media/fat/MiSTer &\n::sysinit:/media/fat/MiSTer &\n::respawn:/sbin/getty\n";

        let edited = ensure_stock_inittab_text(input);

        assert_eq!(edited.matches("::sysinit:/media/fat/MiSTer &").count(), 1);
        assert!(edited.contains("::respawn:/sbin/getty\n"));
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
    fn deploy_transaction_derives_remote_paths_and_local_size() {
        let local = temp_path("deploy-bin");
        fs::write(&local, b"abc").unwrap();

        let tx =
            MagikDeployTransaction::validate(&local, "/media/fat/mister-magik/mister-magik-fb")
                .unwrap();
        let _ = fs::remove_file(&local);

        assert_eq!(tx.remote_dir, "/media/fat/mister-magik");
        assert_eq!(tx.upload, "/media/fat/mister-magik/mister-magik-fb.upload");
        assert_eq!(tx.lock, "/media/fat/mister-magik/deploy.lock");
        assert_eq!(tx.local_bytes, 3);
        assert_eq!(
            tx.chmod_size_verify_command(),
            "chmod +x '/media/fat/mister-magik/mister-magik-fb' && wc -c '/media/fat/mister-magik/mister-magik-fb'"
        );
    }

    #[test]
    fn deploy_transaction_rejects_invalid_remote_paths() {
        let local = temp_path("deploy-invalid-bin");
        fs::write(&local, b"abc").unwrap();

        assert!(MagikDeployTransaction::validate(&local, "relative/path").is_err());
        assert!(MagikDeployTransaction::validate(&local, "/media/fat/mister-magik/").is_err());

        let _ = fs::remove_file(&local);
    }

    #[test]
    fn deploy_size_parsing_reads_busybox_wc_prefix() {
        assert_eq!(
            parse_wc_byte_count("12345 /media/fat/mister-magik/mister-magik-fb\n"),
            Some(12345)
        );
        assert_eq!(parse_wc_byte_count("not-a-size path\n"), None);
    }

    #[test]
    fn agent_deploy_result_verifies_remote_and_size() {
        let result = json!({
            "remote": "/media/fat/mister-magik/mister-magik-fb",
            "remote_bytes": 42
        });

        assert_eq!(
            verify_agent_deploy_result(&result, 42, "/media/fat/mister-magik/mister-magik-fb")
                .unwrap(),
            42
        );
        assert!(
            verify_agent_deploy_result(&result, 43, "/media/fat/mister-magik/mister-magik-fb")
                .is_err()
        );
        assert!(verify_agent_deploy_result(&result, 42, "/tmp/other").is_err());
    }

    #[test]
    fn option_value_reads_next_arg() {
        let args = vec![
            "--settle".to_string(),
            "12".to_string(),
            "--keep-enabled".to_string(),
            "--item".to_string(),
            "first".to_string(),
            "--item".to_string(),
            "second".to_string(),
        ];
        assert_eq!(option_value(&args, "--settle"), Some("12".to_string()));
        assert_eq!(option_value(&args, "--missing"), None);
        assert_eq!(
            option_values(&args, "--item"),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn option_values_do_not_treat_following_flags_as_values() {
        let args = vec![
            "--software-list".to_string(),
            "nes.xml".to_string(),
            "--software-list".to_string(),
            "--software-dir".to_string(),
            "lists".to_string(),
            "--offset".to_string(),
            "-1".to_string(),
            "--out".to_string(),
            "--dry-run".to_string(),
            "--out".to_string(),
            "build/mame.sqlite3".to_string(),
        ];

        assert_eq!(
            option_value(&args, "--software-list"),
            Some("nes.xml".to_string())
        );
        assert_eq!(
            option_values(&args, "--software-list"),
            vec!["nes.xml".to_string()]
        );
        assert_eq!(option_value(&args, "--offset"), Some("-1".to_string()));
        assert_eq!(
            option_value(&args, "--out"),
            Some("build/mame.sqlite3".to_string())
        );
        assert_eq!(option_value(&args, "--missing"), None);
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
        let geometry = FbGeometry {
            width: 2,
            height: 2,
            stride: 8,
            bpp: 32,
        };
        write_png_bgrx_stride(&raw, &geometry, &path).unwrap();
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
        raw_to_png(&raw_path, 2, 2, &png_path, None, 32).unwrap();
        let png = fs::read(&png_path).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));

        let err = raw_to_png(&raw_path, 3, 3, &png_path, None, 32).unwrap_err();
        assert!(err.to_string().contains("expected at least 36"));
        let _ = fs::remove_file(&raw_path);
        let _ = fs::remove_file(&png_path);
    }

    #[test]
    fn raw_to_png_reads_strided_rgb565_file_and_rejects_short_input() {
        let raw_path = temp_path("tiny-rgb565.raw");
        let png_path = temp_path("tiny-rgb565.png");
        fs::write(
            &raw_path,
            [
                0x00, 0xf8, // red in RGB565 little-endian
                0xe0, 0x07, // green
                0xaa, 0xbb, // row padding
                0x1f, 0x00, // blue
                0xff, 0xff, // white
                0xcc, 0xdd, // row padding
            ],
        )
        .unwrap();
        raw_to_png(&raw_path, 2, 2, &png_path, Some(6), 16).unwrap();
        let png = fs::read(&png_path).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));

        let err = raw_to_png(&raw_path, 2, 3, &png_path, Some(6), 16).unwrap_err();
        assert!(err.to_string().contains("expected at least 18"));
        let _ = fs::remove_file(&raw_path);
        let _ = fs::remove_file(&png_path);
    }

    #[test]
    fn raw_to_png_rejects_invalid_framebuffer_geometry() {
        let raw_path = temp_path("invalid-geometry.raw");
        let png_path = temp_path("invalid-geometry.png");
        fs::write(&raw_path, [0u8; 8]).unwrap();

        let err = raw_to_png(&raw_path, 2, 2, &png_path, Some(3), 16).unwrap_err();
        assert!(err.to_string().contains("smaller than packed row"));
        let err = raw_to_png(&raw_path, 2, 2, &png_path, None, 24).unwrap_err();
        assert!(err.to_string().contains("unsupported raw framebuffer bpp"));
        let _ = fs::remove_file(&raw_path);
        let _ = fs::remove_file(&png_path);
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
        let mut machines = parse_mame_listxml(MAME_1942_FIXTURE).unwrap();
        for machine in &mut machines {
            if machine.setname == "1942a" {
                machine.category = Some("Shooter / Vertical".to_string());
            }
        }
        let path = temp_path("mame.sqlite3");
        write_mame_metadata_db(&path, &machines, &[], &[]).unwrap();
        let conn = Connection::open(&path).unwrap();
        let row: (String, String, i64, i64, String) = conn
            .query_row(
                "SELECT parent_setname, manufacturer, rotate, buttons, category FROM mame_machines WHERE setname='1942a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(
            row,
            (
                "1942".to_string(),
                "Capcom".to_string(),
                270,
                2,
                "Shooter / Vertical".to_string()
            )
        );
    }

    #[test]
    fn parses_mame_category_ini_category_section_only() {
        let categories = parse_mame_category_ini(
            r#"
            ; comments are ignored
            [Filenames]
            1942=wrong
            [Category]
              1942 = Shooter / Vertical
            empty =
            # also ignored
            1942a=Shooter / Vertical
            [VerAdded]
            1943=wrong
            "#,
        );

        assert_eq!(categories.len(), 2);
        assert_eq!(
            categories.get("1942").map(String::as_str),
            Some("Shooter / Vertical")
        );
        assert_eq!(
            categories.get("1942a").map(String::as_str),
            Some("Shooter / Vertical")
        );
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
