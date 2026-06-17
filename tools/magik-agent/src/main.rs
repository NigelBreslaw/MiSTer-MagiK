use std::env;

#[cfg(target_os = "linux")]
mod linux {
    use libc::{
        c_char, c_int, c_short, c_ulong, close, if_nametoindex, ifreq, in_addr, ioctl, rtentry,
        sendto, sockaddr, sockaddr_in, sockaddr_ll, socket, AF_INET, AF_PACKET, IFF_UP, IFNAMSIZ,
        RTF_GATEWAY, RTF_UP, SIOCADDRT, SIOCGIFFLAGS, SIOCSIFADDR, SIOCSIFFLAGS, SIOCSIFNETMASK,
        SOCK_DGRAM, SOCK_RAW,
    };
    use serde_json::{json, Value};
    use std::ffi::CString;
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, BufRead, BufReader, Write};
    use std::mem;
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::os::fd::RawFd;
    use std::path::Path;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    const IFACE: &str = "eth0";
    const IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 117);
    const NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
    const GATEWAY: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);
    const AGENT_PORT: u16 = 7497;
    const TOKEN_PATH: &str = "/media/fat/mister-magik/agent.token";
    const LOG: &str = "/tmp/mister-magik-agent.log";
    const PLOG: &str = "/media/fat/mister-magik/bootlogs/agent.log";
    const BOOTLOG_DIR: &str = "/media/fat/mister-magik/bootlogs";
    const SEQ: &str = "/media/fat/mister-magik/bootlogs/agent.seq";
    const ETH_P_ARP: u16 = 0x0806;

    pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
        match args.first().map(String::as_str).unwrap_or("net-boot") {
            "net-boot" => net_boot(),
            "arp" => {
                let mut log = Logger::append(LOG)?;
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
        let mut log = Logger::create(LOG)?;
        let boot_id = next_boot_id();
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
    }

    impl Logger {
        fn create(path: &str) -> io::Result<Self> {
            Ok(Self {
                file: File::create(path)?,
            })
        }

        fn append(path: &str) -> io::Result<Self> {
            Ok(Self {
                file: OpenOptions::new().create(true).append(true).open(path)?,
            })
        }

        fn line(&mut self, msg: String) {
            let _ = writeln!(self.file, "{} agent {msg}", stamp());
            let _ = self.file.flush();
        }
    }

    fn start_control_server(boot_id: u64) {
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
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let peer = stream
            .peer_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|_| "?".to_string());
        append_log_line(format!("control_client peer={peer}"));

        let mut line = String::new();
        let read_result = match stream.try_clone() {
            Ok(cloned) => BufReader::new(cloned).read_line(&mut line),
            Err(err) => Err(err),
        };
        let response = match read_result {
            Ok(0) => response(None, false, None, Some("empty request")),
            Ok(_) => handle_control_line(&line, &token, boot_id, started),
            Err(err) => response(None, false, None, Some(&format!("read error: {err}"))),
        };
        let _ = writeln!(stream, "{response}");
    }

    fn handle_control_line(line: &str, token: &str, boot_id: u64, started: Instant) -> String {
        let parsed: Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(err) => return response(None, false, None, Some(&format!("invalid json: {err}"))),
        };
        let id = parsed.get("id").cloned();
        if parsed.get("token").and_then(Value::as_str) != Some(token) {
            append_log_line("control_auth_failed".to_string());
            return response(id, false, None, Some("unauthorized"));
        }
        let cmd = match parsed.get("cmd").and_then(Value::as_str) {
            Some(cmd) => cmd,
            None => return response(id, false, None, Some("missing cmd")),
        };

        match cmd {
            "ping" => response(id, true, Some(json!({"pong": true})), None),
            "status" => response(id, true, Some(status_json(boot_id, started)), None),
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

    fn append_log_line(msg: String) {
        if let Ok(mut logger) = Logger::append(LOG) {
            logger.line(msg);
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
            Ok(()) => log.line(format!("ifconfig_direct ok iface={iface} ip={ip}")),
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
        route.rt_flags = (RTF_UP | RTF_GATEWAY) as u16;
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
        let stats = read_netdev_stats(IFACE).unwrap_or_default();
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
        let text = fs::read_to_string(LOG).unwrap_or_default();
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(PLOG) {
            let _ = writeln!(
                file,
                "--- agent deferred boot={boot_id} uptime={} ---",
                stamp()
            );
            let _ = write!(file, "{text}");
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
        let mut mac = [0u8; 6];
        for (i, part) in text.trim().split(':').enumerate() {
            if i >= 6 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "too many MAC bytes",
                ));
            }
            mac[i] = u8::from_str_radix(part, 16)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad MAC byte"))?;
        }
        Ok(mac)
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
