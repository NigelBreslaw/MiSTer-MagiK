// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::{json, Value};
use ssh2::Session;
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::Result;

const CACHED_TIMEOUT: Duration = Duration::from_millis(300);
const SCAN_TIMEOUT: Duration = Duration::from_millis(450);
const WORKERS: usize = 64;
const ACCESS_DENIED_MESSAGE: &str =
    "local-network access denied while discovering the MiSTer; rerun with network escalation";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Device {
    pub(crate) address: Ipv4Addr,
    pub(crate) id: String,
}

enum ProbeOutcome {
    Connected(Device),
    Unreachable,
    AccessDenied,
}

struct ScanOutcome {
    found: Vec<Device>,
    access_denied: bool,
}

pub(crate) fn resolve() -> Result<Device> {
    if let Ok(explicit) = env::var("MISTER_IP") {
        let address = explicit.parse()?;
        let device = match probe(address, Duration::from_secs(2)) {
            ProbeOutcome::Connected(device) => device,
            ProbeOutcome::AccessDenied => return Err(ACCESS_DENIED_MESSAGE.into()),
            ProbeOutcome::Unreachable => return Err("configured MiSTer is unavailable".into()),
        };
        save_remembered(&device)?;
        return Ok(device);
    }

    let remembered = load_remembered();
    if let Some(device) = remembered.as_ref() {
        match probe(device.address, CACHED_TIMEOUT) {
            ProbeOutcome::Connected(candidate) if candidate.id == device.id => {
                return Ok(candidate);
            }
            ProbeOutcome::AccessDenied => return Err(ACCESS_DENIED_MESSAGE.into()),
            ProbeOutcome::Connected(_) | ProbeOutcome::Unreachable => {}
        }
    }

    let scan = scan(&local_subnets()?, SCAN_TIMEOUT);
    let selected = select_candidate(remembered.as_ref(), scan)?;
    save_remembered(&selected)?;
    Ok(selected)
}

fn select_candidate(remembered: Option<&Device>, scan: ScanOutcome) -> Result<Device> {
    let ScanOutcome {
        mut found,
        access_denied,
    } = scan;
    if let Some(device) = remembered {
        if let Some(index) = found.iter().position(|candidate| candidate.id == device.id) {
            return Ok(found.remove(index));
        }
    }
    match found.len() {
        0 if access_denied => Err(ACCESS_DENIED_MESSAGE.into()),
        0 => Err("no connected MiSTer found".into()),
        1 => Ok(found.remove(0)),
        count => Err(format!(
            "{count} MiSTers are connected; set MISTER_IP once to select the device"
        )
        .into()),
    }
}

pub(crate) fn state_dir() -> Result<PathBuf> {
    if let Ok(path) = env::var("MISTER_MAGIK_STATE_DIR") {
        return Ok(PathBuf::from(path));
    }
    if let Ok(path) = env::var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("mister-magik"));
    }
    Ok(PathBuf::from(env::var("HOME")?).join(".config/mister-magik"))
}

pub(crate) fn token_path(device_id: &str) -> Result<PathBuf> {
    Ok(state_dir()?
        .join("tokens")
        .join(format!("{}.token", device_id.replace(':', ""))))
}

fn remembered_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("device.json"))
}

fn load_remembered() -> Option<Device> {
    let value: Value =
        serde_json::from_str(&fs::read_to_string(remembered_path().ok()?).ok()?).ok()?;
    Some(Device {
        address: value.get("address")?.as_str()?.parse().ok()?,
        id: normalize_id(value.get("id")?.as_str()?)?,
    })
}

fn save_remembered(device: &Device) -> Result<()> {
    let path = remembered_path()?;
    secure_write(
        &path,
        format!("{}\n", json!({"address": device.address, "id": device.id})).as_bytes(),
    )
}

pub(crate) fn secure_write(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or("state path has no parent")?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let root = state_root_for(path).ok_or("state path is outside the state directory")?;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    fs::write(path, bytes)?;
    Ok(())
}

fn state_root_for(path: &std::path::Path) -> Option<&std::path::Path> {
    let parent = path.parent()?;
    if parent.file_name().and_then(|name| name.to_str()) == Some("tokens") {
        parent.parent()
    } else {
        Some(parent)
    }
}

fn local_subnets() -> Result<Vec<[u8; 3]>> {
    let output = Command::new("ifconfig").output().or_else(|_| {
        Command::new("ip")
            .args(["-o", "-4", "addr", "show"])
            .output()
    })?;
    if !output.status.success() {
        return Err("could not enumerate local network interfaces".into());
    }
    Ok(parse_subnets(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_subnets(text: &str) -> Vec<[u8; 3]> {
    let mut subnets = BTreeSet::new();
    let mut skip_interface = false;
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if !line.starts_with(char::is_whitespace) {
            skip_interface = ["loopback", "docker", "vmware", "tailscale", "utun"]
                .iter()
                .any(|name| lower.contains(name));
        }
        if skip_interface {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        for pair in fields.windows(2).filter(|pair| pair[0] == "inet") {
            let address = pair[1].split('/').next().unwrap_or(pair[1]);
            if let Ok(ip) = address.parse::<Ipv4Addr>() {
                let octets = ip.octets();
                if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() {
                    subnets.insert([octets[0], octets[1], octets[2]]);
                }
            }
        }
    }
    subnets.into_iter().collect()
}

fn scan(subnets: &[[u8; 3]], timeout: Duration) -> ScanOutcome {
    let (job_tx, job_rx) = mpsc::channel::<Ipv4Addr>();
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (found_tx, found_rx) = mpsc::channel();
    let mut workers = Vec::new();
    for _ in 0..WORKERS {
        let jobs = Arc::clone(&job_rx);
        let found = found_tx.clone();
        workers.push(thread::spawn(move || loop {
            let Ok(ip) = jobs.lock().expect("discovery lock poisoned").recv() else {
                break;
            };
            let _ = found.send(probe(ip, timeout));
        }));
    }
    drop(found_tx);
    for subnet in subnets {
        for host in 1..=254 {
            let _ = job_tx.send(Ipv4Addr::new(subnet[0], subnet[1], subnet[2], host));
        }
    }
    drop(job_tx);
    for worker in workers {
        let _ = worker.join();
    }
    let mut access_denied = false;
    let mut found: Vec<_> = found_rx
        .into_iter()
        .filter_map(|outcome| match outcome {
            ProbeOutcome::Connected(device) => Some(device),
            ProbeOutcome::AccessDenied => {
                access_denied = true;
                None
            }
            ProbeOutcome::Unreachable => None,
        })
        .collect();
    found.sort_by_key(|device| device.address);
    found.dedup_by(|a, b| a.id == b.id);
    ScanOutcome {
        found,
        access_denied,
    }
}

fn probe(address: Ipv4Addr, timeout: Duration) -> ProbeOutcome {
    let tcp = match TcpStream::connect_timeout(&SocketAddrV4::new(address, 22).into(), timeout) {
        Ok(tcp) => tcp,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return ProbeOutcome::AccessDenied;
        }
        Err(_) => return ProbeOutcome::Unreachable,
    };
    if tcp.set_read_timeout(Some(timeout)).is_err() || tcp.set_write_timeout(Some(timeout)).is_err()
    {
        return ProbeOutcome::Unreachable;
    }
    let Ok(mut session) = Session::new() else {
        return ProbeOutcome::Unreachable;
    };
    session.set_tcp_stream(tcp);
    if session.handshake().is_err()
        || session
            .userauth_password(
                &env::var("MISTER_USER").unwrap_or_else(|_| "root".into()),
                &env::var("MISTER_PASS").unwrap_or_else(|_| "1".into()),
            )
            .is_err()
    {
        return ProbeOutcome::Unreachable;
    }
    let Ok(mut channel) = session.channel_session() else {
        return ProbeOutcome::Unreachable;
    };
    if channel
        .exec("test -d /media/fat && cat /sys/class/net/eth0/address")
        .is_err()
    {
        return ProbeOutcome::Unreachable;
    }
    let mut output = String::new();
    if channel.read_to_string(&mut output).is_err()
        || channel.wait_close().is_err()
        || channel.exit_status().ok() != Some(0)
    {
        return ProbeOutcome::Unreachable;
    }
    let Some(id) = normalize_id(output.trim()) else {
        return ProbeOutcome::Unreachable;
    };
    ProbeOutcome::Connected(Device { address, id })
}

fn normalize_id(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    let valid = value.len() == 17
        && value.chars().enumerate().all(|(index, ch)| {
            if index % 3 == 2 {
                ch == ':'
            } else {
                ch.is_ascii_hexdigit()
            }
        });
    valid.then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_subnets_without_virtual_or_loopback_interfaces() {
        let text = "lo0: flags=8049<UP,LOOPBACK>\n inet 127.0.0.1\nen0: flags=8863<UP>\n inet 192.168.1.42 netmask 0xffffff00\nutun3: flags=8051<UP>\n inet 10.0.0.2\n2: wlan0 inet 10.23.4.7/24 brd 10.23.4.255\n";
        assert_eq!(parse_subnets(text), vec![[10, 23, 4], [192, 168, 1]]);
    }

    #[test]
    fn validates_device_identity() {
        assert_eq!(
            normalize_id("AA:BB:CC:DD:EE:FF\n").as_deref(),
            Some("aa:bb:cc:dd:ee:ff")
        );
        assert_eq!(normalize_id("not-a-mac"), None);
    }

    #[test]
    fn selection_prefers_remembered_identity_and_rejects_ambiguity() {
        let first = Device {
            address: "192.168.1.10".parse().unwrap(),
            id: "00:11:22:33:44:55".into(),
        };
        let remembered = Device {
            address: "192.168.1.20".parse().unwrap(),
            id: "aa:bb:cc:dd:ee:ff".into(),
        };
        let moved = Device {
            address: "192.168.1.100".parse().unwrap(),
            id: remembered.id.clone(),
        };
        assert_eq!(
            select_candidate(
                Some(&remembered),
                ScanOutcome {
                    found: vec![first.clone(), moved.clone()],
                    access_denied: false,
                }
            )
            .unwrap(),
            moved
        );
        assert!(select_candidate(
            None,
            ScanOutcome {
                found: vec![first, remembered],
                access_denied: false,
            }
        )
        .is_err());
        assert!(select_candidate(
            None,
            ScanOutcome {
                found: Vec::new(),
                access_denied: false,
            }
        )
        .is_err());
    }

    #[test]
    fn access_denied_scan_is_not_reported_as_device_unavailable() {
        let error = select_candidate(
            None,
            ScanOutcome {
                found: Vec::new(),
                access_denied: true,
            },
        )
        .unwrap_err();

        assert_eq!(error.to_string(), ACCESS_DENIED_MESSAGE);
    }

    #[cfg(unix)]
    #[test]
    fn shared_state_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let root =
            std::env::temp_dir().join(format!("mister-magik-state-test-{}", std::process::id()));
        let path = root.join("tokens/device.token");
        secure_write(&path, b"secret\n").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let _ = fs::remove_dir_all(root);
    }
}
