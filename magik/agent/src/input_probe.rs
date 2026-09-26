//! Passive, bounded evdev observation independent of launcher navigation.
use serde_json::{Map, Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

const MAX_EVENTS: usize = 256;
const MAX_DEVICES: usize = 32;
const EVENT_SIZE: usize = if cfg!(target_pointer_width = "64") {
    24
} else {
    16
};

fn event_name(name: &str) -> bool {
    name.strip_prefix("event")
        .is_some_and(|n| !n.is_empty() && n.len() <= 4 && n.bytes().all(|b| b.is_ascii_digit()))
}

fn clock_us() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time);
    }
    time.tv_sec as u64 * 1_000_000 + time.tv_nsec as u64 / 1000
}

fn parse(bytes: &[u8; EVENT_SIZE]) -> (u64, u16, u16, i32) {
    let word = EVENT_SIZE / 2 - 4;
    let number = |offset| {
        if word == 8 {
            i64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap())
        } else {
            i32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap()) as i64
        }
    };
    let at = number(0).max(0) as u64 * 1_000_000 + number(word).max(0) as u64;
    let offset = 2 * word;
    (
        at,
        u16::from_ne_bytes(bytes[offset..offset + 2].try_into().unwrap()),
        u16::from_ne_bytes(bytes[offset + 2..offset + 4].try_into().unwrap()),
        i32::from_ne_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()),
    )
}

fn runtime_snapshot() -> Result<Value, String> {
    use mister_magik_platform_manifest_contract::{Layout, ValidationProfile, parse};
    let status = crate::device::status()?;
    let mut processes = Vec::new();
    for (role, field) in [("main", "pid"), ("launcher", "launcher_pid")] {
        let pid = status[field]
            .as_u64()
            .filter(|pid| *pid > 0)
            .ok_or("missing process identity")?;
        let mut threads = Vec::new();
        for entry in fs::read_dir(format!("/proc/{pid}/task"))
            .map_err(|e| e.to_string())?
            .take(128)
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let stat = fs::read_to_string(entry.path().join("stat")).unwrap_or_default();
            let Some((_, fields)) = stat.rsplit_once(") ") else {
                continue;
            };
            let fields: Vec<_> = fields.split_whitespace().collect();
            let number = |index| {
                fields
                    .get(index)
                    .and_then(|value: &&str| value.parse::<i64>().ok())
            };
            let thread_status = fs::read_to_string(entry.path().join("status")).unwrap_or_default();
            let allowed = thread_status
                .lines()
                .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
                .unwrap_or("")
                .trim();
            threads.push(json!({"tid":entry.file_name().to_string_lossy(),
                "name":fs::read_to_string(entry.path().join("comm")).unwrap_or_default().trim(),
                "nice":number(16),"cpu":number(36),"rt_priority":number(37),"policy":number(38),
                "allowed_cpus":allowed}));
        }
        processes.push(json!({"role":role,"pid":pid,"threads":threads}));
    }
    let layout = Layout::Development;
    let manifest_text = fs::read_to_string(layout.paths().manifest).map_err(|e| e.to_string())?;
    let manifest =
        parse(&manifest_text, layout, ValidationProfile::AgentStrict).map_err(|e| e.to_string())?;
    let main_pid = status["pid"].as_u64().ok_or("missing Main pid")?;
    let main_hash = crate::media::hash(std::path::Path::new(&format!("/proc/{main_pid}/exe")))?;
    let expected = manifest
        .required("main_sha256")
        .map_err(|e| e.to_string())?;
    Ok(
        json!({"processes":processes,"main_revision":manifest.required("main_revision").map_err(|e| e.to_string())?,
        "running_main_matches_manifest":main_hash == expected,"running_main_sha256":main_hash}),
    )
}

pub fn run(fields: &Map<String, Value>) -> Result<Value, String> {
    if fields
        .keys()
        .any(|key| !matches!(key.as_str(), "seconds" | "events"))
    {
        return Err("unexpected input probe field".into());
    }
    let seconds = fields
        .get("seconds")
        .and_then(Value::as_u64)
        .filter(|n| *n <= 30)
        .ok_or("seconds must be an integer from 0 to 30")?;
    let selected = fields
        .get("events")
        .and_then(Value::as_array)
        .ok_or("events must be an array")?;
    if selected.len() > 4 || selected.iter().any(|n| !n.as_str().is_some_and(event_name)) {
        return Err("select at most four eventN devices".into());
    }
    let mut names: Vec<_> = fs::read_dir("/sys/class/input")
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| event_name(name))
        .collect();
    names.sort();
    if names.len() > MAX_DEVICES {
        return Err("too many input devices for bounded inventory".into());
    }
    let mut devices = Vec::new();
    let mut readers: Vec<(usize, File)> = Vec::new();
    for name in names {
        let text = |field: &str| {
            fs::read_to_string(format!("/sys/class/input/{name}/device/{field}"))
                .unwrap_or_default()
                .trim()
                .chars()
                .take(256)
                .collect::<String>()
        };
        let label = text("name");
        let proxy = label == "MiSTer virtual input";
        let mut device = json!({"event":name,"name":label,"proxy":proxy,
            "bus":text("id/bustype"),"vendor":text("id/vendor"),"product":text("id/product"),
            "physical":text("phys"),"capabilities":text("capabilities/ev")});
        if seconds > 0 && (proxy || selected.iter().any(|value| value.as_str() == Some(&name))) {
            let opened = (|| -> Result<File, String> {
                let file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                    .open(format!("/dev/input/{name}"))
                    .map_err(|e| e.to_string())?;
                let clock = libc::CLOCK_MONOTONIC;
                if unsafe { libc::ioctl(file.as_raw_fd(), 0x4004_45a0 as libc::c_ulong, &clock) }
                    < 0
                {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                Ok(file)
            })();
            match opened {
                Ok(file) => {
                    device["observing"] = json!(true);
                    readers.push((devices.len(), file));
                }
                Err(error) => device["error"] = json!(error),
            }
        }
        devices.push(device);
    }
    for requested in selected {
        if !devices.iter().any(|device| device["event"] == *requested) {
            return Err(format!("input device {requested} is absent"));
        }
    }
    let runtime_before = runtime_snapshot().unwrap_or_else(|error| json!({"error":error}));
    let started_at_us = clock_us();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut events = Vec::new();
    let mut truncated = false;
    let mut poll: Vec<_> = readers
        .iter()
        .map(|(_, file)| libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    'capture: while !readers.is_empty() && Instant::now() < deadline {
        if unsafe { libc::poll(poll.as_mut_ptr(), poll.len() as libc::nfds_t, 20) } < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(std::io::Error::last_os_error().to_string());
        }
        for ((device, file), ready) in readers.iter_mut().zip(&poll) {
            if ready.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(format!(
                    "input device {} disconnected or faulted",
                    devices[*device]["event"]
                ));
            }
            if ready.revents & libc::POLLIN == 0 {
                continue;
            }
            loop {
                let mut bytes = [0; EVENT_SIZE];
                match file.read(&mut bytes) {
                    Ok(EVENT_SIZE) => {
                        let (kernel_us, kind, code, value) = parse(&bytes);
                        if kernel_us >= started_at_us
                            && (kind == 1 || kind == 3 || (kind == 0 && code == 3))
                        {
                            events.push(json!({"device":*device,"kernel_us":kernel_us,"read_us":clock_us(),"type":kind,"code":code,"value":value}));
                            if events.len() == MAX_EVENTS {
                                truncated = true;
                                break 'capture;
                            }
                        }
                    }
                    Ok(_) => return Err("short evdev read".into()),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error.to_string()),
                }
                if Instant::now() >= deadline {
                    break 'capture;
                }
            }
        }
    }
    Ok(
        json!({"schema":"input-probe-v1","started_at_us":started_at_us,"ended_at_us":clock_us(),
        "devices":devices,"events":events,"truncated":truncated,
        "runtime_before":runtime_before,
        "runtime":runtime_snapshot().unwrap_or_else(|error| json!({"error":error})),
        "note":"Passive readers never grab devices. An existing exclusive grab may hide raw events; silence does not prove missing controller input."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scheduling_mutations_are_rejected_before_device_access() {
        let fields = json!({"seconds": 20, "events": [], "ordinary_launcher": true});
        assert_eq!(
            run(fields.as_object().unwrap()).unwrap_err(),
            "unexpected input probe field"
        );
    }
    #[test]
    fn device_selection_cannot_escape_input_nodes() {
        for name in ["event0", "event123"] {
            assert!(event_name(name));
        }
        for name in ["event", "../event0", "event0/../../etc", "event-1"] {
            assert!(!event_name(name));
        }
    }
    #[test]
    fn parser_preserves_release_timestamp_and_signed_axis() {
        let mut bytes = [0; EVENT_SIZE];
        let word = EVENT_SIZE / 2 - 4;
        if word == 8 {
            bytes[..8].copy_from_slice(&12i64.to_ne_bytes());
            bytes[8..16].copy_from_slice(&34000i64.to_ne_bytes());
        } else {
            bytes[..4].copy_from_slice(&12i32.to_ne_bytes());
            bytes[4..8].copy_from_slice(&34000i32.to_ne_bytes());
        }
        bytes[word * 2..word * 2 + 2].copy_from_slice(&3u16.to_ne_bytes());
        bytes[word * 2 + 2..word * 2 + 4].copy_from_slice(&16u16.to_ne_bytes());
        bytes[word * 2 + 4..].copy_from_slice(&(-1i32).to_ne_bytes());
        assert_eq!(parse(&bytes), (12_034_000, 3, 16, -1));
    }
}
