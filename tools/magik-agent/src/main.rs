use std::env;

#[cfg(any(target_os = "linux", test))]
use std::io;

#[cfg(any(target_os = "linux", test))]
fn parse_mac_text(text: &str) -> io::Result<[u8; 6]> {
    let mut mac = [0u8; 6];
    let mut parts = text.trim().split(':');
    for byte in &mut mac {
        let part = parts
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "too few MAC bytes"))?;
        *byte = u8::from_str_radix(part, 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad MAC byte"))?;
    }
    if parts.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "too many MAC bytes",
        ));
    }
    Ok(mac)
}

#[cfg(any(target_os = "linux", test))]
mod sd_browse {
    use serde_json::{json, Value};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Instant, UNIX_EPOCH};

    pub const ROOT_PATH: &str = "/";
    pub const SD_ROOT: &str = "/media/fat";

    pub fn list_dir_at_root(
        root: &Path,
        requested_path: &str,
        show_hidden: bool,
    ) -> Result<Value, String> {
        let start = Instant::now();
        let relative_path = normalize_sd_relative_path(requested_path)?;
        let host_path = sd_host_path(root, &relative_path);
        let mut entries = Vec::new();
        for entry in
            fs::read_dir(&host_path).map_err(|err| format!("read_dir {relative_path}: {err}"))?
        {
            let entry = entry.map_err(|err| format!("read_dir {relative_path}: {err}"))?;
            if !show_hidden && is_hidden_name(&entry.file_name().to_string_lossy()) {
                continue;
            }
            entries.push(sd_entry_json(&relative_path, entry)?);
        }
        entries.sort_by(sd_entry_value_cmp);
        Ok(json!({
            "schema": "mister-magik-sd-list-dir-v1",
            "path": relative_path,
            "show_hidden": show_hidden,
            "entries": entries,
            "elapsed_ms": start.elapsed().as_millis() as u64,
        }))
    }

    pub fn normalize_sd_relative_path(path: &str) -> Result<String, String> {
        let trimmed = path.trim();
        if trimmed.is_empty() || trimmed == ROOT_PATH {
            return Ok(ROOT_PATH.to_string());
        }
        if trimmed.starts_with("/media/fat/") || trimmed == SD_ROOT {
            return Err("sd path must be relative to /media/fat".to_string());
        }
        let mut parts = Vec::new();
        for part in trimmed.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." {
                return Err("sd path may not contain ..".to_string());
            }
            if part.contains('\0') {
                return Err("sd path may not contain NUL".to_string());
            }
            parts.push(part);
        }
        if parts.is_empty() {
            Ok(ROOT_PATH.to_string())
        } else {
            Ok(format!("/{}", parts.join("/")))
        }
    }

    pub fn sd_host_path(root: &Path, relative_path: &str) -> PathBuf {
        let mut path = root.to_path_buf();
        for part in relative_path.split('/').filter(|part| !part.is_empty()) {
            path.push(part);
        }
        path
    }

    pub fn sd_entry_json(parent_path: &str, entry: fs::DirEntry) -> Result<Value, String> {
        let name = entry.file_name().to_string_lossy().to_string();
        let entry_path = child_sd_path(parent_path, &name);
        let file_type = entry
            .file_type()
            .map_err(|err| format!("file_type {entry_path}: {err}"))?;
        let metadata = entry
            .metadata()
            .map_err(|err| format!("metadata {entry_path}: {err}"))?;
        let kind = if file_type.is_dir() {
            "directory"
        } else {
            "file"
        };
        let modified_unix_ms = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        Ok(json!({
            "name": name,
            "path": entry_path,
            "kind": kind,
            "size": if file_type.is_dir() { 0 } else { metadata.len() },
            "modified_unix_ms": modified_unix_ms,
            "readonly": metadata.permissions().readonly(),
            "hidden": is_hidden_name(&name),
        }))
    }

    fn is_hidden_name(name: &str) -> bool {
        name.starts_with('.')
    }

    pub fn child_sd_path(parent_path: &str, name: &str) -> String {
        if parent_path == ROOT_PATH {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        }
    }

    pub fn sd_entry_value_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
        let a_dir = a.get("kind").and_then(Value::as_str) == Some("directory");
        let b_dir = b.get("kind").and_then(Value::as_str) == Some("directory");
        b_dir
            .cmp(&a_dir)
            .then_with(|| natural_name_cmp(entry_name(a), entry_name(b)))
    }

    fn entry_name(value: &Value) -> &str {
        value.get("name").and_then(Value::as_str).unwrap_or("")
    }

    fn natural_name_cmp(a: &str, b: &str) -> std::cmp::Ordering {
        let mut a_chars = a.char_indices().peekable();
        let mut b_chars = b.char_indices().peekable();
        loop {
            match (a_chars.peek().copied(), b_chars.peek().copied()) {
                (None, None) => return std::cmp::Ordering::Equal,
                (None, Some(_)) => return std::cmp::Ordering::Less,
                (Some(_), None) => return std::cmp::Ordering::Greater,
                (Some((_, ac)), Some((_, bc))) if ac.is_ascii_digit() && bc.is_ascii_digit() => {
                    let a_digits = take_ascii_digits(a, &mut a_chars);
                    let b_digits = take_ascii_digits(b, &mut b_chars);
                    let a_trimmed = a_digits.trim_start_matches('0');
                    let b_trimmed = b_digits.trim_start_matches('0');
                    let a_number = if a_trimmed.is_empty() { "0" } else { a_trimmed };
                    let b_number = if b_trimmed.is_empty() { "0" } else { b_trimmed };
                    let by_len = a_number.len().cmp(&b_number.len());
                    if by_len != std::cmp::Ordering::Equal {
                        return by_len;
                    }
                    let by_value = a_number.cmp(b_number);
                    if by_value != std::cmp::Ordering::Equal {
                        return by_value;
                    }
                    let by_raw_len = a_digits.len().cmp(&b_digits.len());
                    if by_raw_len != std::cmp::Ordering::Equal {
                        return by_raw_len;
                    }
                }
                (Some((_, ac)), Some((_, bc))) => {
                    a_chars.next();
                    b_chars.next();
                    let by_char = ac
                        .to_ascii_lowercase()
                        .cmp(&bc.to_ascii_lowercase())
                        .then_with(|| ac.cmp(&bc));
                    if by_char != std::cmp::Ordering::Equal {
                        return by_char;
                    }
                }
            }
        }
    }

    fn take_ascii_digits<'a>(
        text: &'a str,
        chars: &mut std::iter::Peekable<std::str::CharIndices<'a>>,
    ) -> &'a str {
        let start = chars.peek().map(|(index, _)| *index).unwrap_or(text.len());
        let mut end = start;
        while let Some((index, ch)) = chars.peek().copied() {
            if !ch.is_ascii_digit() {
                break;
            }
            end = index + ch.len_utf8();
            chars.next();
        }
        &text[start..end]
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use libc::{
        c_char, c_int, c_short, c_ulong, close, if_nametoindex, ifreq, in_addr, ioctl, rtentry,
        sendto, sockaddr, sockaddr_in, sockaddr_ll, socket, AF_INET, AF_PACKET, IFF_UP, IFNAMSIZ,
        RTF_GATEWAY, RTF_UP, SIOCADDRT, SIOCGIFFLAGS, SIOCSIFADDR, SIOCSIFFLAGS, SIOCSIFNETMASK,
        SOCK_DGRAM, SOCK_RAW,
    };
    use serde_json::{json, Value};
    use std::collections::VecDeque;
    use std::ffi::CString;
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::mem;
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::os::fd::RawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    const IFACE: &str = "eth0";
    const IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 117);
    const NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
    const GATEWAY: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);
    const AGENT_PORT: u16 = 7498;
    const TOKEN_PATH: &str = "/media/fat/mister-magik/agent.token";
    // Temporary while the host/device file-transfer auth flow is being reworked.
    const CONTROL_AUTH_DISABLED: bool = true;
    const LOG: &str = "/tmp/mister-magik-agent.log";
    const PLOG: &str = "/media/fat/mister-magik/bootlogs/agent.log";
    const BOOTLOG_DIR: &str = "/media/fat/mister-magik/bootlogs";
    const SEQ: &str = "/media/fat/mister-magik/bootlogs/agent.seq";
    const CRASH_DIR: &str = "/media/fat/mister-magik/crashes";
    const LATEST_CRASH_REPORT: &str = "/media/fat/mister-magik/crashes/latest.json";
    const ETH_P_ARP: u16 = 0x0806;
    const LOG_RING_CAPACITY: usize = 512;
    const TIMELINE_CAPACITY: usize = 128;
    const MAX_DEPLOY_BYTES: u64 = 64 * 1024 * 1024;

    type SharedLogRing = Arc<Mutex<LogRing>>;
    type SharedTimeline = Arc<Mutex<Timeline>>;

    static LOG_RING: OnceLock<SharedLogRing> = OnceLock::new();
    static TIMELINE: OnceLock<SharedTimeline> = OnceLock::new();

    pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
        match args.first().map(String::as_str).unwrap_or("net-boot") {
            "net-boot" => net_boot(),
            "arp" => {
                let mut log = Logger::append(LOG, fresh_log_ring())?;
                send_gratuitous_arp(IFACE, IP, &mut log)?;
                Ok(())
            }
            "-h" | "--help" => {
                eprintln!("usage: mister-magik-agent [net-boot|arp]");
                Ok(())
            }
            other => Err(format!("unknown command: {other}").into()),
        }
    }

    fn net_boot() -> Result<(), Box<dyn std::error::Error>> {
        let ring = fresh_log_ring();
        let _ = LOG_RING.set(Arc::clone(&ring));
        let timeline = fresh_timeline();
        let _ = TIMELINE.set(timeline);
        let mut log = Logger::create(LOG, ring)?;
        let boot_id = next_boot_id();
        timeline_record_once(
            "agent_start",
            format!("boot={boot_id} pid={}", std::process::id()),
        );
        log.line(format!(
            "worker_start boot={boot_id} pid={}",
            std::process::id()
        ));
        start_control_server(boot_id);

        for _ in 0..80 {
            configure_network(IFACE, IP, NETMASK, GATEWAY, &mut log);
            let _ = send_gratuitous_arp(IFACE, IP, &mut log);
            let carrier = read_trimmed("/sys/class/net/eth0/carrier").unwrap_or_else(|| "?".into());
            let operstate =
                read_trimmed("/sys/class/net/eth0/operstate").unwrap_or_else(|| "?".into());
            log.line(format!(
                "configured carrier={carrier} operstate={operstate}"
            ));
            if carrier == "1" {
                timeline_record_once("carrier_up", format!("operstate={operstate}"));
                log.line(format!("carrier_ready boot={boot_id}"));
                configure_network(IFACE, IP, NETMASK, GATEWAY, &mut log);
                for _ in 0..3 {
                    let _ = send_gratuitous_arp(IFACE, IP, &mut log);
                }
                for _ in 0..40 {
                    snapshot(boot_id, &mut log);
                    thread::sleep(Duration::from_secs(1));
                }
                persist_log(boot_id, &mut log);
                park_forever();
            }
            thread::sleep(Duration::from_millis(250));
        }

        log.line("gave_up".to_string());
        persist_log(boot_id, &mut log);
        park_forever();
    }

    fn park_forever() -> ! {
        loop {
            thread::sleep(Duration::from_secs(3600));
        }
    }

    struct Logger {
        file: File,
        ring: SharedLogRing,
    }

    impl Logger {
        fn create(path: &str, ring: SharedLogRing) -> io::Result<Self> {
            Ok(Self {
                file: File::create(path)?,
                ring,
            })
        }

        fn append(path: &str, ring: SharedLogRing) -> io::Result<Self> {
            Ok(Self {
                file: OpenOptions::new().create(true).append(true).open(path)?,
                ring,
            })
        }

        fn line(&mut self, msg: String) {
            let line = format!("{} agent {msg}", stamp());
            record_log_line(&self.ring, &line);
            let _ = writeln!(self.file, "{line}");
            let _ = self.file.flush();
        }

        fn ring_text(&self) -> String {
            ring_lines(&self.ring).join("\n")
        }
    }

    struct LogRing {
        lines: VecDeque<String>,
        dropped: u64,
    }

    impl LogRing {
        fn new() -> Self {
            Self {
                lines: VecDeque::with_capacity(LOG_RING_CAPACITY),
                dropped: 0,
            }
        }

        fn push(&mut self, line: String) {
            if self.lines.len() == LOG_RING_CAPACITY {
                self.lines.pop_front();
                self.dropped += 1;
            }
            self.lines.push_back(line);
        }
    }

    fn fresh_log_ring() -> SharedLogRing {
        Arc::new(Mutex::new(LogRing::new()))
    }

    fn record_log_line(ring: &SharedLogRing, line: &str) {
        if let Ok(mut ring) = ring.lock() {
            ring.push(line.to_string());
        }
    }

    fn ring_lines(ring: &SharedLogRing) -> Vec<String> {
        ring.lock()
            .map(|ring| ring.lines.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn log_ring_json() -> Value {
        match LOG_RING.get().and_then(|ring| ring.lock().ok()) {
            Some(ring) => json!({
                "capacity": LOG_RING_CAPACITY,
                "dropped": ring.dropped,
                "count": ring.lines.len(),
                "lines": ring.lines.iter().cloned().collect::<Vec<_>>(),
            }),
            None => json!({
                "capacity": LOG_RING_CAPACITY,
                "dropped": 0,
                "count": 0,
                "lines": [],
            }),
        }
    }

    struct Timeline {
        events: Vec<TimelineEvent>,
        dropped: u64,
    }

    struct TimelineEvent {
        name: String,
        uptime_ms: u64,
        detail: String,
    }

    impl Timeline {
        fn new() -> Self {
            Self {
                events: Vec::with_capacity(TIMELINE_CAPACITY),
                dropped: 0,
            }
        }

        fn record_once(&mut self, name: &str, detail: String) {
            if self.events.iter().any(|event| event.name == name) {
                return;
            }
            if self.events.len() == TIMELINE_CAPACITY {
                self.events.remove(0);
                self.dropped += 1;
            }
            self.events.push(TimelineEvent {
                name: name.to_string(),
                uptime_ms: uptime_ms_now(),
                detail,
            });
        }
    }

    fn fresh_timeline() -> SharedTimeline {
        Arc::new(Mutex::new(Timeline::new()))
    }

    fn timeline_record_once(name: &str, detail: String) {
        if let Some(timeline) = TIMELINE.get() {
            if let Ok(mut timeline) = timeline.lock() {
                timeline.record_once(name, detail);
            }
        }
    }

    fn timeline_json(boot_id: u64, started: Instant) -> Value {
        match TIMELINE.get().and_then(|timeline| timeline.lock().ok()) {
            Some(timeline) => json!({
                "boot_id": boot_id,
                "agent_uptime_ms": started.elapsed().as_millis() as u64,
                "capacity": TIMELINE_CAPACITY,
                "dropped": timeline.dropped,
                "count": timeline.events.len(),
                "events": timeline.events.iter().map(|event| {
                    json!({
                        "event": event.name,
                        "uptime_ms": event.uptime_ms,
                        "detail": event.detail,
                    })
                }).collect::<Vec<_>>(),
            }),
            None => json!({
                "boot_id": boot_id,
                "agent_uptime_ms": started.elapsed().as_millis() as u64,
                "capacity": TIMELINE_CAPACITY,
                "dropped": 0,
                "count": 0,
                "events": [],
            }),
        }
    }

    fn start_control_server(boot_id: u64) {
        let token = if CONTROL_AUTH_DISABLED {
            append_log_line(
                "control_auth_disabled accepting unauthenticated TCP commands".to_string(),
            );
            String::new()
        } else {
            let token = match fs::read_to_string(TOKEN_PATH) {
                Ok(token) => token.trim().to_string(),
                Err(err) => {
                    append_log_line(format!("control_token_missing path={TOKEN_PATH} err={err}"));
                    return;
                }
            };
            if token.is_empty() {
                append_log_line(format!("control_token_empty path={TOKEN_PATH}"));
                return;
            }
            token
        };

        thread::spawn(move || {
            let started = Instant::now();
            let token = Arc::new(token);
            let listener = match TcpListener::bind(("0.0.0.0", AGENT_PORT)) {
                Ok(listener) => listener,
                Err(err) => {
                    append_log_line(format!("control_bind_error port={AGENT_PORT} err={err}"));
                    return;
                }
            };
            append_log_line(format!("control_listen port={AGENT_PORT} boot={boot_id}"));
            timeline_record_once("control_listen", format!("port={AGENT_PORT}"));

            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let token = Arc::clone(&token);
                        thread::spawn(move || {
                            handle_control_client(stream, token, boot_id, started)
                        });
                    }
                    Err(err) => append_log_line(format!("control_accept_error err={err}")),
                }
            }
        });
    }

    fn handle_control_client(
        mut stream: TcpStream,
        token: Arc<String>,
        boot_id: u64,
        started: Instant,
    ) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
        let peer = stream
            .peer_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|_| "?".to_string());
        timeline_record_once("first_client_connect", format!("peer={peer}"));
        append_log_line(format!("control_client peer={peer}"));

        let mut reader = match stream.try_clone() {
            Ok(cloned) => BufReader::new(cloned),
            Err(err) => {
                let response = response(None, false, None, Some(&format!("clone error: {err}")));
                let _ = writeln!(stream, "{response}");
                return;
            }
        };
        let mut line = String::new();
        let read_result = reader.read_line(&mut line);
        let response = match read_result {
            Ok(0) => response(None, false, None, Some("empty request")),
            Ok(_) => handle_control_line(&line, &token, boot_id, started, &mut reader),
            Err(err) => response(None, false, None, Some(&format!("read error: {err}"))),
        };
        let _ = writeln!(stream, "{response}");
    }

    fn handle_control_line<R: Read>(
        line: &str,
        token: &str,
        boot_id: u64,
        started: Instant,
        reader: &mut R,
    ) -> String {
        let parsed: Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(err) => return response(None, false, None, Some(&format!("invalid json: {err}"))),
        };
        let id = parsed.get("id").cloned();
        if !CONTROL_AUTH_DISABLED && parsed.get("token").and_then(Value::as_str) != Some(token) {
            append_log_line("control_auth_failed".to_string());
            return response(id, false, None, Some("unauthorized"));
        }
        let cmd = match parsed.get("cmd").and_then(Value::as_str) {
            Some(cmd) => cmd,
            None => return response(id, false, None, Some("missing cmd")),
        };
        let args = parsed.get("args").cloned().unwrap_or_else(|| json!({}));
        timeline_record_once("first_command", format!("cmd={cmd}"));

        match cmd {
            "ping" => response(id, true, Some(json!({"pong": true})), None),
            "status" => response(id, true, Some(status_json(boot_id, started)), None),
            "logs" => response(id, true, Some(log_ring_json()), None),
            "timeline" => response(id, true, Some(timeline_json(boot_id, started)), None),
            "diagnostics" => response(id, true, Some(diagnostics_json(boot_id, started)), None),
            "deploy_magik_bin" => match deploy_magik_bin(args) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => response(id, false, None, Some(&err)),
            },
            "deploy_magik_bin_stream" => match deploy_magik_bin_stream(args, reader) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => response(id, false, None, Some(&err)),
            },
            "magik" => match magik_control(args) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => response(id, false, None, Some(&err)),
            },
            "sd_list_dir" => match sd_list_dir(args) {
                Ok(result) => response(id, true, Some(result), None),
                Err(err) => response(id, false, None, Some(&err)),
            },
            "reboot" => match schedule_reboot(args) {
                Ok(mode) => response(
                    id,
                    true,
                    Some(json!({"scheduled": true, "mode": mode})),
                    None,
                ),
                Err(err) => response(id, false, None, Some(&err)),
            },
            _ => response(id, false, None, Some("unknown cmd")),
        }
    }

    fn response(id: Option<Value>, ok: bool, result: Option<Value>, error: Option<&str>) -> String {
        let value = if ok {
            json!({"id": id.unwrap_or(Value::Null), "ok": true, "result": result.unwrap_or(Value::Null)})
        } else {
            json!({"id": id.unwrap_or(Value::Null), "ok": false, "error": error.unwrap_or("error")})
        };
        value.to_string()
    }

    fn status_json(boot_id: u64, started: Instant) -> Value {
        json!({
            "agent": {
                "version": env!("CARGO_PKG_VERSION"),
                "boot_id": boot_id,
                "uptime_ms": started.elapsed().as_millis() as u64,
                "port": AGENT_PORT,
            },
            "network": {
                "interface": IFACE,
                "ip": IP.to_string(),
                "carrier": read_trimmed("/sys/class/net/eth0/carrier"),
                "operstate": read_trimmed("/sys/class/net/eth0/operstate"),
                "mac": read_trimmed("/sys/class/net/eth0/address"),
                "stats": read_netdev_stats_value(IFACE),
                "routes": read_routes(),
                "arp": read_arp_entries(),
            },
            "processes": {
                "sshd": read_pid_list("sshd"),
                "MiSTer_MagiK": read_pid_list("MiSTer_MagiK"),
                "mister-magik-fb": read_pid_list("mister-magik-fb"),
            },
            "system": {
                "uptime": read_trimmed("/proc/uptime"),
            }
        })
    }

    fn diagnostics_json(boot_id: u64, started: Instant) -> Value {
        json!({
            "schema": "mister-magik-agent-diagnostics-v1",
            "collected_uptime_ms": uptime_ms_now(),
            "status": status_json(boot_id, started),
            "timeline": timeline_json(boot_id, started),
            "agent_logs": log_ring_json(),
            "net": {
                "carrier": read_text_value("/sys/class/net/eth0/carrier"),
                "operstate": read_text_value("/sys/class/net/eth0/operstate"),
                "address": read_text_value("/sys/class/net/eth0/address"),
                "route": read_text_value("/proc/net/route"),
                "arp": read_text_value("/proc/net/arp"),
                "dev": read_text_value("/proc/net/dev"),
            },
            "processes": {
                "ps": command_text_value("ps", &["w"]),
                "sshd": read_pid_list("sshd"),
                "MiSTer_MagiK": read_pid_list("MiSTer_MagiK"),
                "mister-magik-fb": read_pid_list("mister-magik-fb"),
            },
            "files": {
                "slint_status": read_text_value("/tmp/mister-magik/status.json"),
                "main_status": read_text_value("/tmp/mister-magik/main-status.json"),
                "events_tail": tail_text_value("/tmp/mister-magik/events.jsonl", 80),
                "slint_log_tail": tail_text_value("/tmp/mister-magik-slint.log", 120),
                "main_log_tail": tail_text_value("/tmp/mister-magik-main.log", 120),
                "agent_tmp_log_tail": tail_text_value(LOG, 160),
                "agent_persistent_log_tail": tail_text_value(PLOG, 160),
                "boot_analytics_tail": tail_text_value("/tmp/mister-magik-boot-analytics.tsv", 80),
            },
            "crashes": crash_reports_json(),
        })
    }

    fn magik_control(args: Value) -> Result<Value, String> {
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("status");
        match action {
            "status" => Ok(magik_status_json(action, None, None)),
            "suspend" | "resume" | "restart-launcher" => magik_fifo_action(action),
            _ => Err(format!("unsupported magik action: {action}")),
        }
    }

    fn sd_list_dir(args: Value) -> Result<Value, String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(crate::sd_browse::ROOT_PATH);
        let show_hidden = args
            .get("show_hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        crate::sd_browse::list_dir_at_root(Path::new(crate::sd_browse::SD_ROOT), path, show_hidden)
    }

    fn deploy_magik_bin(args: Value) -> Result<Value, String> {
        let remote = args
            .get("remote")
            .and_then(Value::as_str)
            .unwrap_or("/media/fat/mister-magik/mister-magik-fb");
        validate_deploy_remote(remote)?;
        let expectations = deploy_expectations(&args)?;
        let hex = args
            .get("data_hex")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing deploy data".to_string())?;

        let decode_t = Instant::now();
        let payload = decode_hex(hex)?;
        let bytes = decode_deploy_payload(&expectations, payload)?;
        deploy_magik_bin_bytes(
            remote,
            &expectations,
            bytes,
            "hex",
            decode_t.elapsed().as_millis() as u64,
        )
    }

    fn deploy_magik_bin_stream(args: Value, reader: &mut dyn Read) -> Result<Value, String> {
        let remote = args
            .get("remote")
            .and_then(Value::as_str)
            .unwrap_or("/media/fat/mister-magik/mister-magik-fb");
        validate_deploy_remote(remote)?;
        let expectations = deploy_expectations(&args)?;
        let receive_t = Instant::now();
        let mut payload = vec![0u8; expectations.payload_size as usize];
        reader
            .read_exact(&mut payload)
            .map_err(|err| err.to_string())?;
        let receive_ms = receive_t.elapsed().as_millis() as u64;
        let decode_t = Instant::now();
        let bytes = decode_deploy_payload(&expectations, payload)?;
        let decode_ms = decode_t.elapsed().as_millis() as u64;
        deploy_magik_bin_bytes(remote, &expectations, bytes, "stream", receive_ms).map(
            |mut result| {
                if let Some(object) = result.as_object_mut() {
                    object.insert("decode_ms".to_string(), json!(decode_ms));
                }
                result
            },
        )
    }

    struct DeployExpectations {
        raw_size: u64,
        payload_size: u64,
        checksum: String,
        encoding: String,
    }

    fn deploy_expectations(args: &Value) -> Result<DeployExpectations, String> {
        let raw_size = args
            .get("size")
            .and_then(Value::as_u64)
            .ok_or_else(|| "missing deploy size".to_string())?;
        if raw_size > MAX_DEPLOY_BYTES {
            return Err(format!(
                "deploy size {raw_size} exceeds max {MAX_DEPLOY_BYTES}"
            ));
        }
        let encoding = args
            .get("encoding")
            .and_then(Value::as_str)
            .unwrap_or("raw")
            .to_string();
        match encoding.as_str() {
            "raw" | "lz4-block" => {}
            _ => return Err(format!("unsupported deploy encoding: {encoding}")),
        }
        let payload_size = args
            .get("payload_size")
            .and_then(Value::as_u64)
            .unwrap_or(raw_size);
        if payload_size > MAX_DEPLOY_BYTES {
            return Err(format!(
                "deploy payload size {payload_size} exceeds max {MAX_DEPLOY_BYTES}"
            ));
        }
        if encoding == "raw" && payload_size != raw_size {
            return Err(format!(
                "raw payload size mismatch expected={raw_size} payload={payload_size}"
            ));
        }
        let checksum = args
            .get("checksum")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing deploy checksum".to_string())?;
        Ok(DeployExpectations {
            raw_size,
            payload_size,
            checksum: checksum.to_string(),
            encoding,
        })
    }

    fn decode_deploy_payload(
        expectations: &DeployExpectations,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        if payload.len() as u64 != expectations.payload_size {
            return Err(format!(
                "deploy payload size mismatch expected={} actual={}",
                expectations.payload_size,
                payload.len()
            ));
        }
        match expectations.encoding.as_str() {
            "raw" => Ok(payload),
            "lz4-block" => lz4_flex::block::decompress(&payload, expectations.raw_size as usize)
                .map_err(|err| err.to_string()),
            _ => Err(format!(
                "unsupported deploy encoding: {}",
                expectations.encoding
            )),
        }
    }

    fn deploy_magik_bin_bytes(
        remote: &str,
        expectations: &DeployExpectations,
        bytes: Vec<u8>,
        transport: &str,
        receive_ms: u64,
    ) -> Result<Value, String> {
        let total_t = Instant::now();
        if bytes.len() as u64 != expectations.raw_size {
            return Err(format!(
                "deploy size mismatch expected={} actual={}",
                expectations.raw_size,
                bytes.len()
            ));
        }
        let checksum = fnv64_hex(&bytes);
        if checksum != expectations.checksum {
            return Err(format!(
                "deploy checksum mismatch expected={} actual={checksum}",
                expectations.checksum
            ));
        }

        append_log_line(format!(
            "deploy_magik_bin_start remote={remote} bytes={} payload_bytes={} encoding={} checksum={}",
            expectations.raw_size,
            expectations.payload_size,
            expectations.encoding,
            expectations.checksum
        ));
        let suspend_t = Instant::now();
        magik_fifo_action("suspend")?;
        let suspend_ms = suspend_t.elapsed().as_millis() as u64;

        let swap_t = Instant::now();
        let swap_result = deploy_bytes_to_remote(&bytes, remote);
        let swap_ms = swap_t.elapsed().as_millis() as u64;
        if let Err(err) = swap_result {
            let _ = magik_fifo_action("resume");
            append_log_line(format!("deploy_magik_bin_error remote={remote} err={err}"));
            return Err(err);
        }

        let resume_t = Instant::now();
        let resume = magik_fifo_action("resume")?;
        let resume_ms = resume_t.elapsed().as_millis() as u64;
        let remote_bytes = fs::metadata(remote).map(|meta| meta.len()).unwrap_or(0);
        append_log_line(format!(
            "deploy_magik_bin_done remote={remote} bytes={remote_bytes} checksum={}",
            expectations.checksum
        ));
        Ok(json!({
            "transport": transport,
            "encoding": expectations.encoding,
            "remote": remote,
            "bytes": expectations.raw_size,
            "payload_bytes": expectations.payload_size,
            "remote_bytes": remote_bytes,
            "checksum": expectations.checksum,
            "receive_ms": receive_ms,
            "suspend_ms": suspend_ms,
            "swap_ms": swap_ms,
            "resume_ms": resume_ms,
            "total_ms": total_t.elapsed().as_millis() as u64,
            "resume": resume,
        }))
    }

    fn validate_deploy_remote(remote: &str) -> Result<(), String> {
        if !remote.starts_with("/media/fat/mister-magik/") {
            return Err("deploy remote must be under /media/fat/mister-magik".to_string());
        }
        if remote.ends_with('/') || remote.contains('\0') || remote.contains("/../") {
            return Err(format!("unsupported deploy remote: {remote}"));
        }
        Ok(())
    }

    fn deploy_bytes_to_remote(bytes: &[u8], remote: &str) -> Result<(), String> {
        let remote_path = Path::new(remote);
        let parent = remote_path
            .parent()
            .ok_or_else(|| "deploy remote has no parent".to_string())?;
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        let upload = parent.join(format!(
            ".{}.upload",
            remote_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("mister-magik-fb")
        ));
        let _ = fs::remove_file(&upload);
        fs::write(&upload, bytes).map_err(|err| err.to_string())?;
        fs::set_permissions(&upload, fs::Permissions::from_mode(0o755))
            .map_err(|err| err.to_string())?;
        fs::rename(&upload, remote).map_err(|err| err.to_string())?;
        Ok(())
    }

    fn magik_fifo_action(action: &str) -> Result<Value, String> {
        let command = match action {
            "suspend" => "mister_magik_suspend",
            "resume" => "mister_magik_resume",
            "restart-launcher" => "mister_magik_restart_launcher",
            _ => return Err(format!("unsupported magik action: {action}")),
        };

        let before_main = pid_string("MiSTer_MagiK");
        let before_launcher = pid_string("mister-magik-fb");
        append_log_line(format!(
            "magik_command action={action} command={command} main_pids={before_main} launcher_pids={before_launcher}"
        ));

        if !Path::new("/dev/MiSTer_cmd").exists() {
            let err = "missing /dev/MiSTer_cmd".to_string();
            append_log_line(format!("magik_command_error action={action} err={err}"));
            return Err(err);
        }
        if before_main.is_empty() {
            let err = "MiSTer_MagiK is not running".to_string();
            append_log_line(format!("magik_command_error action={action} err={err}"));
            return Err(err);
        }

        fs::write("/dev/MiSTer_cmd", format!("{command}\n")).map_err(|err| {
            append_log_line(format!("magik_command_error action={action} err={err}"));
            err.to_string()
        })?;

        let settle_ms = if action == "suspend" { 400 } else { 1500 };
        thread::sleep(Duration::from_millis(settle_ms));
        let after_main = pid_string("MiSTer_MagiK");
        let after_launcher = pid_string("mister-magik-fb");
        if before_main != after_main || before_launcher != after_launcher {
            append_log_line(format!(
                "magik_pid_change action={action} main_before={before_main} main_after={after_main} launcher_before={before_launcher} launcher_after={after_launcher}"
            ));
        }
        append_log_line(format!(
            "magik_command_done action={action} command={command} main_pids={after_main} launcher_pids={after_launcher}"
        ));
        Ok(magik_status_json(
            action,
            Some(command.to_string()),
            Some(settle_ms),
        ))
    }

    fn magik_status_json(action: &str, command: Option<String>, settle_ms: Option<u64>) -> Value {
        let main_pids = read_pid_list("MiSTer_MagiK");
        let launcher_pids = read_pid_list("mister-magik-fb");
        let main_status = read_json_value("/tmp/mister-magik/main-status.json");
        let slint_status = read_json_value("/tmp/mister-magik/status.json");
        let slint_status_current = status_pid_matches(&slint_status, &launcher_pids);
        json!({
            "action": action,
            "command": command,
            "settle_ms": settle_ms,
            "processes": {
                "MiSTer_MagiK": main_pids,
                "mister-magik-fb": launcher_pids,
            },
            "files": {
                "main_status": main_status,
                "slint_status": slint_status,
                "slint_status_current": slint_status_current,
            }
        })
    }

    fn read_text_value(path: &str) -> Value {
        fs::read_to_string(path)
            .map(Value::String)
            .unwrap_or(Value::Null)
    }

    fn read_json_value(path: &str) -> Value {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null)
    }

    fn crash_reports_json() -> Value {
        let recent = recent_crash_report_paths(5)
            .into_iter()
            .map(|path| {
                let path_text = path.to_string_lossy().to_string();
                let report = fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or(Value::Null);
                json!({
                    "path": path_text,
                    "report": report,
                })
            })
            .collect::<Vec<_>>();
        json!({
            "dir": CRASH_DIR,
            "latest_path": LATEST_CRASH_REPORT,
            "latest": read_json_value(LATEST_CRASH_REPORT),
            "recent": recent,
        })
    }

    fn recent_crash_report_paths(limit: usize) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(CRASH_DIR) else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        let latest_report_id = read_json_value(LATEST_CRASH_REPORT)
            .get("report_id")
            .and_then(Value::as_str)
            .map(|report_id| format!("{report_id}.json"));
        if let Some(name) = latest_report_id.as_deref() {
            paths.push(Path::new(CRASH_DIR).join(name));
        }
        let mut other_paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("report-")
                            && name.ends_with(".json")
                            && Some(name) != latest_report_id.as_deref()
                    })
            })
            .collect::<Vec<_>>();
        other_paths.sort();
        paths.extend(other_paths.into_iter().rev());
        paths.into_iter().take(limit).collect::<Vec<_>>()
    }

    fn status_pid_matches(status: &Value, pids: &Value) -> bool {
        let Some(status_pid) = status.get("pid").and_then(Value::as_u64) else {
            return false;
        };
        pids.as_array()
            .is_some_and(|pids| pids.iter().any(|pid| pid.as_u64() == Some(status_pid)))
    }

    fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
        if !hex.len().is_multiple_of(2) {
            return Err("hex payload has odd length".to_string());
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        let raw = hex.as_bytes();
        let mut i = 0;
        while i < raw.len() {
            let hi = hex_value(raw[i])?;
            let lo = hex_value(raw[i + 1])?;
            bytes.push((hi << 4) | lo);
            i += 2;
        }
        Ok(bytes)
    }

    fn hex_value(byte: u8) -> Result<u8, String> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            b'A'..=b'F' => Ok(byte - b'A' + 10),
            _ => Err(format!("invalid hex byte: {byte}")),
        }
    }

    fn fnv64_hex(bytes: &[u8]) -> String {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }

    fn tail_text_value(path: &str, n: usize) -> Value {
        let Ok(text) = fs::read_to_string(path) else {
            return Value::Null;
        };
        let lines: Vec<_> = text.lines().collect();
        let start = lines.len().saturating_sub(n);
        Value::String(lines[start..].join("\n"))
    }

    fn command_text_value(program: &str, args: &[&str]) -> Value {
        match std::process::Command::new(program).args(args).output() {
            Ok(output) => {
                let mut text = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.trim().is_empty() {
                    text.push_str("\n[stderr]\n");
                    text.push_str(&stderr);
                }
                Value::String(text)
            }
            Err(err) => Value::String(format!("error: {err}")),
        }
    }

    fn schedule_reboot(args: Value) -> Result<String, String> {
        let mode = args
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("raw")
            .to_string();
        match mode.as_str() {
            "raw" | "supervised" => {}
            _ => return Err(format!("unsupported reboot mode: {mode}")),
        }
        let thread_mode = mode.clone();
        append_persistent_log_line(format!("reboot_scheduled mode={mode}"));
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let result = if thread_mode == "supervised" {
                fs::write("/dev/MiSTer_cmd", "mister_magik_reboot\n").map_err(|err| err.to_string())
            } else {
                std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg("nohup /sbin/reboot >/dev/null 2>&1 &")
                    .spawn()
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            };
            if let Err(err) = result {
                append_log_line(format!(
                    "reboot_schedule_error mode={thread_mode} err={err}"
                ));
            }
        });
        Ok(mode)
    }

    fn read_routes() -> Value {
        let routes: Vec<Value> = fs::read_to_string("/proc/net/route")
            .ok()
            .map(|text| {
                text.lines()
                    .skip(1)
                    .map(|line| {
                        let fields: Vec<_> = line.split_whitespace().collect();
                        json!({
                            "iface": fields.first().copied().unwrap_or(""),
                            "destination": fields.get(1).copied().unwrap_or(""),
                            "gateway": fields.get(2).copied().unwrap_or(""),
                            "flags": fields.get(3).copied().unwrap_or(""),
                            "metric": fields.get(6).copied().unwrap_or(""),
                            "mask": fields.get(7).copied().unwrap_or(""),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Value::Array(routes)
    }

    fn read_arp_entries() -> Value {
        let entries: Vec<Value> = fs::read_to_string("/proc/net/arp")
            .ok()
            .map(|text| {
                text.lines()
                    .skip(1)
                    .map(|line| {
                        let fields: Vec<_> = line.split_whitespace().collect();
                        json!({
                            "ip": fields.first().copied().unwrap_or(""),
                            "flags": fields.get(2).copied().unwrap_or(""),
                            "mac": fields.get(3).copied().unwrap_or(""),
                            "device": fields.get(5).copied().unwrap_or(""),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Value::Array(entries)
    }

    fn read_netdev_stats_value(iface: &str) -> Value {
        match read_netdev_stats_fields(iface) {
            Some(fields) => json!({
                "rx_bytes": fields[0],
                "rx_packets": fields[1],
                "tx_bytes": fields[8],
                "tx_packets": fields[9],
            }),
            None => Value::Null,
        }
    }

    fn read_pid_list(name: &str) -> Value {
        let pids: Vec<Value> = read_pidof(name)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|pid| pid.parse::<u64>().ok())
            .map(Value::from)
            .collect();
        Value::Array(pids)
    }

    fn pid_string(name: &str) -> String {
        read_pidof(name).unwrap_or_default().replace(' ', ",")
    }

    fn append_log_line(msg: String) {
        let line = format!("{} agent {msg}", stamp());
        if let Some(ring) = LOG_RING.get() {
            record_log_line(ring, &line);
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(LOG) {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }

    fn append_persistent_log_line(msg: String) {
        append_log_line(msg.clone());
        let _ = fs::create_dir_all(BOOTLOG_DIR);
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(PLOG) {
            let _ = writeln!(file, "{} agent {msg}", stamp());
            let _ = file.flush();
            let _ = file.sync_all();
        }
    }

    fn configure_network(
        iface: &str,
        ip: Ipv4Addr,
        netmask: Ipv4Addr,
        gateway: Ipv4Addr,
        log: &mut Logger,
    ) {
        match configure_interface(iface, ip, netmask) {
            Ok(()) => {
                timeline_record_once("ip_configured", format!("iface={iface} ip={ip}"));
                log.line(format!("ifconfig_direct ok iface={iface} ip={ip}"));
            }
            Err(err) => log.line(format!("ifconfig_direct err={err}")),
        }
        match add_default_route(iface, gateway) {
            Ok(RouteStatus::Added) => log.line(format!("route_direct added gw={gateway}")),
            Ok(RouteStatus::Exists) => log.line(format!("route_direct exists gw={gateway}")),
            Err(err) => log.line(format!("route_direct err={err}")),
        }
    }

    fn configure_interface(iface: &str, ip: Ipv4Addr, netmask: Ipv4Addr) -> io::Result<()> {
        let fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            set_ifaddr(fd, iface, SIOCSIFADDR, ip)?;
            set_ifaddr(fd, iface, SIOCSIFNETMASK, netmask)?;
            let mut flags_req = new_ifreq(iface)?;
            cvt_ioctl(unsafe { ioctl(fd, SIOCGIFFLAGS as c_ulong, &mut flags_req) })?;
            let flags = unsafe { flags_req.ifr_ifru.ifru_flags };
            flags_req.ifr_ifru.ifru_flags = flags | IFF_UP as c_short;
            cvt_ioctl(unsafe { ioctl(fd, SIOCSIFFLAGS as c_ulong, &flags_req) })?;
            Ok(())
        })();
        unsafe {
            close(fd);
        }
        result
    }

    fn set_ifaddr(fd: RawFd, iface: &str, request: c_ulong, addr: Ipv4Addr) -> io::Result<()> {
        let mut req = new_ifreq(iface)?;
        req.ifr_ifru.ifru_addr = sockaddr_from_ipv4(addr);
        cvt_ioctl(unsafe { ioctl(fd, request, &req) })
    }

    enum RouteStatus {
        Added,
        Exists,
    }

    fn add_default_route(iface: &str, gateway: Ipv4Addr) -> io::Result<RouteStatus> {
        let fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let dev = CString::new(iface).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "interface contains NUL byte")
        })?;
        let mut route: rtentry = unsafe { mem::zeroed() };
        route.rt_gateway = sockaddr_from_ipv4(gateway);
        route.rt_dst = sockaddr_from_ipv4(Ipv4Addr::new(0, 0, 0, 0));
        route.rt_genmask = sockaddr_from_ipv4(Ipv4Addr::new(0, 0, 0, 0));
        route.rt_flags = RTF_UP | RTF_GATEWAY;
        route.rt_dev = dev.as_ptr() as *mut c_char;

        let rc = unsafe { ioctl(fd, SIOCADDRT as c_ulong, &route) };
        let status = if rc == 0 {
            Ok(RouteStatus::Added)
        } else {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EEXIST) {
                Ok(RouteStatus::Exists)
            } else {
                Err(err)
            }
        };
        unsafe {
            close(fd);
        }
        status
    }

    fn send_gratuitous_arp(iface: &str, ip: Ipv4Addr, log: &mut Logger) -> io::Result<()> {
        let mac = read_mac("/sys/class/net/eth0/address")?;
        let ifname = CString::new(iface)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad interface name"))?;
        let ifindex = unsafe { if_nametoindex(ifname.as_ptr()) };
        if ifindex == 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { socket(AF_PACKET, SOCK_RAW, htons(ETH_P_ARP) as c_int) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut sent = 0;
        let result = (|| {
            for opcode in [1u16, 2u16] {
                let frame = arp_frame(mac, ip, opcode);
                let mut addr: sockaddr_ll = unsafe { mem::zeroed() };
                addr.sll_family = AF_PACKET as libc::sa_family_t;
                addr.sll_protocol = htons(ETH_P_ARP);
                addr.sll_ifindex = ifindex as c_int;
                addr.sll_halen = 6;
                addr.sll_addr[..6].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
                let rc = unsafe {
                    sendto(
                        fd,
                        frame.as_ptr().cast(),
                        frame.len(),
                        0,
                        (&addr as *const sockaddr_ll).cast::<sockaddr>(),
                        mem::size_of::<sockaddr_ll>() as u32,
                    )
                };
                if rc < 0 {
                    return Err(io::Error::last_os_error());
                }
                sent += 1;
            }
            Ok(())
        })();
        unsafe {
            close(fd);
        }
        if result.is_ok() {
            timeline_record_once("raw_arp_sent", format!("iface={iface} ip={ip} sent={sent}"));
            log.line(format!("gratuitous_arp sent={sent}"));
        }
        result
    }

    fn arp_frame(mac: [u8; 6], ip: Ipv4Addr, opcode: u16) -> [u8; 42] {
        let mut frame = [0u8; 42];
        frame[0..6].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        frame[6..12].copy_from_slice(&mac);
        frame[12..14].copy_from_slice(&ETH_P_ARP.to_be_bytes());
        frame[14..16].copy_from_slice(&1u16.to_be_bytes());
        frame[16..18].copy_from_slice(&0x0800u16.to_be_bytes());
        frame[18] = 6;
        frame[19] = 4;
        frame[20..22].copy_from_slice(&opcode.to_be_bytes());
        frame[22..28].copy_from_slice(&mac);
        frame[28..32].copy_from_slice(&ip.octets());
        if opcode == 2 {
            frame[32..38].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        }
        frame[38..42].copy_from_slice(&ip.octets());
        frame
    }

    fn snapshot(boot_id: u64, log: &mut Logger) {
        let carrier = read_trimmed("/sys/class/net/eth0/carrier").unwrap_or_else(|| "?".into());
        let operstate = read_trimmed("/sys/class/net/eth0/operstate").unwrap_or_else(|| "?".into());
        let sshd_pid = read_pidof("sshd").unwrap_or_else(|| "none".into());
        if sshd_pid != "none" {
            timeline_record_once("sshd_seen", format!("pid={sshd_pid}"));
        }
        if let Some(pid) = read_pidof("MiSTer_MagiK") {
            timeline_record_once("magik_main_seen", format!("pid={pid}"));
        }
        if let Some(pid) = read_pidof("mister-magik-fb") {
            timeline_record_once("magik_launcher_seen", format!("pid={pid}"));
        }
        let stats = read_netdev_stats(IFACE).unwrap_or_default();
        if let Some(fields) = read_netdev_stats_fields(IFACE) {
            if fields[1] > 0 {
                timeline_record_once(
                    "first_rx",
                    format!("rx_bytes={} rx_packets={}", fields[0], fields[1]),
                );
            }
            if fields[9] > 0 {
                timeline_record_once(
                    "first_tx",
                    format!("tx_bytes={} tx_packets={}", fields[8], fields[9]),
                );
            }
        }
        log.line(format!(
            "snapshot boot={boot_id} carrier={carrier} operstate={operstate} sshd_pid={sshd_pid} {stats}"
        ));
        if let Some(route) = read_trimmed("/proc/net/route") {
            for line in route.lines().take(4) {
                log.line(format!("route {line}"));
            }
        }
    }

    fn read_netdev_stats(iface: &str) -> Option<String> {
        let fields = read_netdev_stats_fields(iface)?;
        Some(format!(
            "rx_bytes={} rx_packets={} tx_bytes={} tx_packets={}",
            fields[0], fields[1], fields[8], fields[9]
        ))
    }

    fn read_netdev_stats_fields(iface: &str) -> Option<[u64; 16]> {
        let text = fs::read_to_string("/proc/net/dev").ok()?;
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix(&format!("{iface}:")) {
                let fields: Vec<u64> = rest
                    .split_whitespace()
                    .filter_map(|field| field.parse().ok())
                    .collect();
                if fields.len() >= 16 {
                    let mut values = [0u64; 16];
                    values.copy_from_slice(&fields[..16]);
                    return Some(values);
                }
            }
        }
        None
    }

    fn persist_log(boot_id: u64, log: &mut Logger) {
        thread::sleep(Duration::from_secs(20));
        let _ = fs::create_dir_all(BOOTLOG_DIR);
        let text = log.ring_text();
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(PLOG) {
            let _ = writeln!(
                file,
                "--- agent deferred boot={boot_id} uptime={} ---",
                stamp()
            );
            let _ = writeln!(file, "{text}");
        }
        log.line(format!("persisted boot={boot_id}"));
    }

    fn next_boot_id() -> u64 {
        let _ = fs::create_dir_all(BOOTLOG_DIR);
        let n = fs::read_to_string(SEQ)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        let _ = fs::write(SEQ, n.to_string());
        n
    }

    fn read_pidof(name: &str) -> Option<String> {
        let output = std::process::Command::new("pidof")
            .arg(name)
            .output()
            .ok()?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
        None
    }

    fn read_trimmed(path: &str) -> Option<String> {
        fs::read_to_string(path).ok().map(|s| s.trim().to_string())
    }

    fn read_mac(path: &str) -> io::Result<[u8; 6]> {
        let text = fs::read_to_string(Path::new(path))?;
        super::parse_mac_text(&text)
    }

    fn new_ifreq(iface: &str) -> io::Result<ifreq> {
        let mut req: ifreq = unsafe { mem::zeroed() };
        let bytes = iface.as_bytes();
        if bytes.len() >= IFNAMSIZ {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "interface name too long",
            ));
        }
        for (dst, src) in req.ifr_name.iter_mut().zip(bytes.iter()) {
            *dst = *src as c_char;
        }
        Ok(req)
    }

    fn sockaddr_from_ipv4(ip: Ipv4Addr) -> sockaddr {
        let mut sin: sockaddr_in = unsafe { mem::zeroed() };
        sin.sin_family = AF_INET as libc::sa_family_t;
        sin.sin_addr = in_addr {
            s_addr: u32::from(ip).to_be(),
        };
        unsafe { mem::transmute::<sockaddr_in, sockaddr>(sin) }
    }

    fn cvt_ioctl(rc: c_int) -> io::Result<()> {
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn htons(value: u16) -> u16 {
        value.to_be()
    }

    fn stamp() -> String {
        fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_whitespace().next().map(str::to_string))
            .unwrap_or_else(|| "?".into())
    }

    fn uptime_ms_now() -> u64 {
        fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
            .map(|secs| (secs * 1000.0) as u64)
            .unwrap_or(0)
    }
}

#[cfg(not(target_os = "linux"))]
mod linux {
    pub fn run(_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
        Err("mister-magik-agent can only run on Linux/MiSTer".into())
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if let Err(err) = linux::run(&args) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_text_parser_accepts_exact_six_octets() {
        assert_eq!(
            parse_mac_text("aa:BB:01:23:45:ff\n").unwrap(),
            [0xaa, 0xbb, 0x01, 0x23, 0x45, 0xff]
        );
    }

    #[test]
    fn mac_text_parser_rejects_wrong_octet_count_and_bad_hex() {
        assert_eq!(
            parse_mac_text("aa:bb:cc:dd:ee").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            parse_mac_text("aa:bb:cc:dd:ee:ff:00").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            parse_mac_text("aa:bb:cc:dd:ee:zz").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn sd_relative_paths_normalize_and_reject_escapes() {
        assert_eq!(sd_browse::normalize_sd_relative_path("").unwrap(), "/");
        assert_eq!(
            sd_browse::normalize_sd_relative_path("///games//NES/./").unwrap(),
            "/games/NES"
        );
        assert_eq!(
            sd_browse::normalize_sd_relative_path("/_Arcade").unwrap(),
            "/_Arcade"
        );
        assert!(sd_browse::normalize_sd_relative_path("../secret").is_err());
        assert!(sd_browse::normalize_sd_relative_path("/games/../secret").is_err());
        assert!(sd_browse::normalize_sd_relative_path("/media/fat").is_err());
        assert!(sd_browse::normalize_sd_relative_path("/media/fat/games").is_err());
    }

    #[test]
    fn sd_host_path_stays_under_root_after_normalization() {
        let root = std::path::Path::new("/media/fat");
        assert_eq!(sd_browse::sd_host_path(root, "/"), root);
        assert_eq!(
            sd_browse::sd_host_path(root, "/games/NES"),
            std::path::PathBuf::from("/media/fat/games/NES")
        );
    }

    #[test]
    fn sd_list_dir_sorts_and_classifies_entries() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-agent-sd-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("folder10")).unwrap();
        std::fs::create_dir_all(root.join("folder2")).unwrap();
        std::fs::write(root.join("file10.rom"), b"0123456789").unwrap();
        std::fs::write(root.join("file2.rom"), b"12").unwrap();
        std::fs::write(root.join(".hidden"), b"h").unwrap();
        std::fs::write(root.join("readonly.txt"), b"r").unwrap();
        let mut readonly_permissions = std::fs::metadata(root.join("readonly.txt"))
            .unwrap()
            .permissions();
        readonly_permissions.set_readonly(true);
        std::fs::set_permissions(root.join("readonly.txt"), readonly_permissions).unwrap();

        let visible = sd_browse::list_dir_at_root(&root, "/", false).unwrap();
        let entries = visible["entries"].as_array().unwrap();

        let names = entries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "folder2",
                "folder10",
                "file2.rom",
                "file10.rom",
                "readonly.txt"
            ]
        );
        assert_eq!(visible["show_hidden"], false);
        assert_eq!(entries[0]["kind"], "directory");
        assert_eq!(entries[2]["size"], 2);
        assert_eq!(entries[4]["readonly"], true);

        let hidden = sd_browse::list_dir_at_root(&root, "/", true).unwrap();
        let hidden_entries = hidden["entries"].as_array().unwrap();
        let hidden_names = hidden_entries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            hidden_names,
            vec![
                "folder2",
                "folder10",
                ".hidden",
                "file2.rom",
                "file10.rom",
                "readonly.txt"
            ]
        );
        assert_eq!(hidden["show_hidden"], true);
        assert_eq!(hidden_entries[2]["hidden"], true);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sd_list_dir_returns_expected_json_shape() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-agent-sd-json-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("games")).unwrap();
        std::fs::write(root.join("MiSTer.ini"), b"ini").unwrap();

        let result = sd_browse::list_dir_at_root(&root, "/", false).unwrap();
        assert_eq!(result["schema"], "mister-magik-sd-list-dir-v1");
        assert_eq!(result["path"], "/");
        assert_eq!(result["show_hidden"], false);
        assert_eq!(result["entries"][0]["name"], "games");
        assert_eq!(result["entries"][0]["kind"], "directory");
        assert_eq!(result["entries"][1]["name"], "MiSTer.ini");
        assert_eq!(result["entries"][1]["kind"], "file");

        let err = sd_browse::list_dir_at_root(&root, "/missing", false).unwrap_err();
        assert!(err.contains("read_dir /missing"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_agent_reports_platform_error() {
        let err = linux::run(&[]).unwrap_err().to_string();
        assert!(err.contains("only run on Linux/MiSTer"));
    }
}
