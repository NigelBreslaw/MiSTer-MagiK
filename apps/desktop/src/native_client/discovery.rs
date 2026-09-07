use super::{AgentError, wire};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
const CAPS: &[&str] = &[
    "status",
    "device-identity-v1",
    "dashboard-status",
    "sd-browser",
    "framebuffer-stream",
    "telemetry-stream",
    "capture-framebuffer",
];
static PREPARATION: Mutex<bool> = Mutex::new(false);
static SESSION: Mutex<Option<Client>> = Mutex::new(None);
#[derive(Clone)]
pub struct Client {
    pub address: String,
    pub identity: String,
    pub(super) token: String,
}
pub fn state_root() -> PathBuf {
    if let Ok(path) = std::env::var("MISTER_MAGIK2_STATE") {
        return expand(&path);
    }
    let root = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| expand("~/.local/state"));
    root.join("mister-magik2")
}
fn expand(path: &str) -> PathBuf {
    if path == "~" {
        return PathBuf::from(std::env::var("HOME").unwrap_or_default());
    }
    if let Some(tail) = path.strip_prefix("~/") {
        return PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(tail);
    }
    PathBuf::from(path)
}
fn identity(s: &str) -> Result<String, AgentError> {
    let s = s.to_ascii_lowercase();
    if s.split(':').count() != 6
        || !s
            .split(':')
            .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()))
        || s == "00:00:00:00:00:00"
        || s == "ff:ff:ff:ff:ff:ff"
    {
        return Err(AgentError::Protocol("invalid device identity".into()));
    }
    Ok(s)
}
fn identify(address: &str, timeout: Duration) -> Result<String, AgentError> {
    let deadline = Instant::now() + timeout;
    let ip = address
        .parse::<Ipv4Addr>()
        .map_err(|_| AgentError::Command("MiSTer address must be IPv4".into()))?;
    let mut stream = TcpStream::connect_timeout(&SocketAddrV4::new(ip, 7500).into(), timeout)
        .map_err(|error| {
            if matches!(error.kind(), std::io::ErrorKind::PermissionDenied) {
                AgentError::Command("Local-network permission denied".into())
            } else {
                AgentError::from(error)
            }
        })?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    wire::write(&mut stream, "identify", "identify", "", json!({}))?;
    let (response, body) = wire::read(
        &mut super::Deadline {
            stream: &mut stream,
            deadline,
        },
        "identify",
        "identified",
    )?;
    if !body.is_empty() {
        return Err(AgentError::Protocol(
            "identification body unexpected".into(),
        ));
    }
    identity(response["device_identity"].as_str().unwrap_or(""))
}
fn token_path(id: &str) -> PathBuf {
    state_root().join(format!(
        "{}.token",
        Sha256::digest(id.as_bytes())[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}
impl Client {
    pub fn open(address: &str) -> Result<Self, AgentError> {
        let mut session = SESSION.lock().unwrap();
        if let Some(client) = session.as_ref()
            && (address.is_empty() || address == client.address)
        {
            return Ok(client.clone());
        }
        let client = Self::prepare(address)?;
        *session = Some(client.clone());
        Ok(client)
    }
    pub fn rediscover(address: &str) -> Result<Self, AgentError> {
        let mut session = SESSION.lock().unwrap();
        *session = None;
        let client = Self::resolve(address)?;
        *session = Some(client.clone());
        Ok(client)
    }
    fn prepare(address: &str) -> Result<Self, AgentError> {
        match Self::resolve(address) {
            Ok(client) => {
                let status = client.request("status", json!({}))?;
                if CAPS.iter().all(|cap| {
                    status["capabilities"]
                        .as_array()
                        .is_some_and(|caps| caps.contains(&json!(cap)))
                }) {
                    return Ok(client);
                }
            }
            Err(AgentError::Unauthorized) => return Err(AgentError::Unauthorized),
            Err(AgentError::Command(detail)) if detail != "Native token is missing" => {
                return Err(AgentError::Command(detail));
            }
            Err(error @ AgentError::Protocol(_)) => return Err(error),
            Err(_) => {}
        }
        let mut attempted = PREPARATION.lock().unwrap();
        if *attempted {
            return Err(AgentError::Command(
                "Native setup failed; run scripts/magik2 desktop-prepare --json and retry".into(),
            ));
        }
        *attempted = true;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut command = Command::new(root.join("scripts/magik2"));
        command
            .args(["desktop-prepare", "--json"])
            .current_dir(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if !address.is_empty() {
            command.env("MISTER_IP", address);
        }
        let output = bounded_command(command, Duration::from_secs(300))?;
        let prepared: Value = serde_json::from_slice(&output)
            .map_err(|e| AgentError::Protocol(format!("setup response: {e}")))?;
        if prepared["outcome"] != "ready" {
            return Err(AgentError::Command("native preparation failed".into()));
        }
        let client = Self::resolve(address)?;
        if prepared["identity"] != client.identity {
            return Err(AgentError::Protocol("prepared identity mismatch".into()));
        }
        Ok(client)
    }
    fn resolve(address: &str) -> Result<Self, AgentError> {
        let root = state_root();
        let profile = match fs::read(root.join("device.json")) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                .map_err(|e| AgentError::Protocol(e.to_string()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
            Err(e) => return Err(e.into()),
        };
        if !profile.is_null()
            && (!profile.as_object().is_some_and(|v| v.len() == 3)
                || ["identity", "address", "username"].iter().any(|key| {
                    !profile[*key]
                        .as_str()
                        .is_some_and(|value| !value.is_empty())
                }))
        {
            return Err(AgentError::Protocol(
                "invalid remembered MiSTer configuration".into(),
            ));
        }
        let remembered = profile["identity"].as_str().map(identity).transpose()?;
        let explicit = !address.is_empty();
        let initial = if !address.is_empty() {
            Some(address)
        } else {
            profile["address"].as_str()
        };
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut found = Vec::new();
        if let Some(candidate) = initial {
            match identify(candidate, Duration::from_millis(400)) {
                Ok(id) if remembered.as_ref().is_none_or(|v| v == &id) => {
                    found.push((candidate.to_string(), id))
                }
                Ok(_) if explicit => {
                    return Err(AgentError::Command(
                        "Selected address belongs to a different MiSTer; use device select".into(),
                    ));
                }
                Err(error @ AgentError::Command(_)) => return Err(error),
                _ => {}
            }
        }
        if found.is_empty() {
            if !address.is_empty() {
                return Err(AgentError::Unreachable(
                    "Selected MiSTer is unreachable".into(),
                ));
            }
            let candidates = candidates();
            let next = AtomicUsize::new(0);
            let results = Mutex::new(Vec::new());
            let errors = Mutex::new(Vec::new());
            std::thread::scope(|scope| {
                for _ in 0..32 {
                    scope.spawn(|| {
                        while Instant::now() < deadline {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some(address) = candidates.get(i) else {
                                break;
                            };
                            match identify(
                                address,
                                Duration::from_millis(350)
                                    .min(deadline.saturating_duration_since(Instant::now())),
                            ) {
                                Ok(id) if remembered.as_ref().is_none_or(|v| v == &id) => {
                                    results.lock().unwrap().push((address.clone(), id))
                                }
                                Err(error @ AgentError::Command(_)) => {
                                    errors.lock().unwrap().push(error);
                                    break;
                                }
                                _ => {}
                            }
                        }
                    });
                }
            });
            found = results.into_inner().unwrap();
            if let Some(error) = errors.into_inner().unwrap().pop() {
                return Err(error);
            }
        }
        found.sort();
        found.dedup_by(|a, b| a.1 == b.1);
        if found.len() > 1 {
            return Err(AgentError::Command(format!(
                "Multiple MiSTers found: {}. Select with scripts/magik2 device select ADDRESS",
                found
                    .iter()
                    .map(|v| v.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let (address, id) = found.pop().ok_or_else(|| {
            AgentError::Unreachable("No selected MiSTer found within eight seconds".into())
        })?;
        let token = fs::read_to_string(token_path(&id))?.trim().to_string();
        if token.is_empty() {
            return Err(AgentError::Command("Native token is missing".into()));
        }
        let client = Self {
            address: address.clone(),
            identity: id.clone(),
            token,
        };
        let status = client.request("status", json!({}))?;
        if status["device_identity"] != id {
            return Err(AgentError::Protocol(
                "authenticated identity mismatch".into(),
            ));
        }
        fs::create_dir_all(&root)?;
        let tmp = root.join(format!(".device-desktop-{}", std::process::id()));
        let result = (|| {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            f.write_all(serde_json::to_string(&json!({"identity":id,"address":address,"username":profile["username"].as_str().unwrap_or("root")})).unwrap().as_bytes())?;
            fs::rename(&tmp, root.join("device.json"))
        })();
        let _ = fs::remove_file(tmp);
        result?;
        Ok(client)
    }
    pub(super) fn connect(&self, timeout: Duration) -> Result<TcpStream, AgentError> {
        let ip = self
            .address
            .parse::<Ipv4Addr>()
            .map_err(|e| AgentError::Protocol(e.to_string()))?;
        let stream = TcpStream::connect_timeout(&SocketAddrV4::new(ip, 7500).into(), timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        Ok(stream)
    }
}
fn bounded_command(mut command: Command, limit: Duration) -> Result<Vec<u8>, AgentError> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.stdout(Stdio::piped()).spawn()?;
    #[cfg(unix)]
    let pid = child.id();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.take(65537).read_to_end(&mut bytes).map(|_| bytes);
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + limit;
    let result = (|| {
        loop {
            if let Some(status) = child.try_wait()? {
                let bytes = rx.recv_timeout(Duration::from_secs(1)).map_err(|_| {
                    AgentError::Unreachable("command output did not close".into())
                })??;
                if bytes.len() > 65536 {
                    return Err(AgentError::Protocol("command output exceeded limit".into()));
                }
                if !status.success() {
                    let detail = serde_json::from_slice::<Value>(&bytes)
                        .ok()
                        .and_then(|value| value["detail"].as_str().map(str::to_string))
                        .unwrap_or_else(|| format!("Preparation/local discovery failed: {status}"));
                    return Err(AgentError::Command(detail));
                }
                return Ok(bytes);
            }
            if Instant::now() >= deadline {
                return Err(AgentError::Unreachable("command deadline exceeded".into()));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            // SAFETY: this process group was created above for this child only.
            unsafe {
                kill(-(pid as i32), 9);
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
fn candidates() -> Vec<String> {
    let mut found = Vec::new();
    for (program, args) in [
        (
            "/usr/bin/dscacheutil",
            vec!["-q", "host", "-a", "name", "mister.local"],
        ),
        (
            "/usr/bin/dscacheutil",
            vec!["-q", "host", "-a", "name", "mister"],
        ),
        ("/usr/sbin/arp", vec!["-an"]),
        ("/sbin/ifconfig", vec![]),
    ] {
        let mut command = Command::new(program);
        command.args(args).stderr(Stdio::null());
        let Ok(bytes) = bounded_command(command, Duration::from_millis(500)) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        if program.ends_with("ifconfig") {
            found.extend(subnet_candidates(&text));
        } else {
            for word in text.split_whitespace() {
                if let Ok(ip) = word.trim_matches(['(', ')']).parse::<Ipv4Addr>()
                    && ip.is_private()
                {
                    found.push(ip.to_string());
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    found.retain(|ip| seen.insert(ip.clone()));
    found.truncate(512);
    found
}

fn subnet_candidates(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.first() != Some(&"inet") {
            continue;
        }
        let Some(ip) = fields
            .get(1)
            .and_then(|v| v.parse::<Ipv4Addr>().ok())
            .filter(|ip| ip.is_private())
        else {
            continue;
        };
        let Some(mask) = fields
            .iter()
            .position(|v| *v == "netmask")
            .and_then(|i| fields.get(i + 1))
            .and_then(|v| {
                v.strip_prefix("0x")
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| v.parse::<Ipv4Addr>().ok().map(u32::from))
            })
        else {
            continue;
        };
        if (!mask).wrapping_add(1) & !mask != 0 {
            continue;
        }
        let mask = mask | 0xffffff00;
        let host = u32::from(ip);
        let start = host & mask;
        let end = start | !mask;
        for address in start.saturating_add(1)..end {
            if address != host {
                out.push(Ipv4Addr::from(address).to_string());
            }
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subnet_search_is_private_and_respects_narrow_networks() {
        assert!(subnet_candidates("inet 8.8.8.8 netmask 0xffffff00").is_empty());
        assert_eq!(
            subnet_candidates("inet 192.168.1.1 netmask 0xfffffffc"),
            ["192.168.1.2"]
        );
        assert!(subnet_candidates("inet 192.168.1.1 netmask 0xffffffff").is_empty());
        assert_eq!(
            subnet_candidates("inet 10.0.0.1 netmask 0xff000000").len(),
            253
        );
    }
    #[test]
    fn identity_and_token_name_match_python() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../magik2/agent/tests/fixtures/desktop-wire.json"
        ))
        .unwrap();
        assert_eq!(identity("AA:BB:CC:DD:EE:FF").unwrap(), fixture["identity"]);
        assert_eq!(
            token_path(fixture["identity"].as_str().unwrap())
                .file_name()
                .unwrap()
                .to_str()
                .unwrap(),
            fixture["token_file"]
        );
        for bad in ["", "00:00:00:00:00:00", "ff:ff:ff:ff:ff:ff", "not-a-device"] {
            assert!(identity(bad).is_err());
        }
    }
}
