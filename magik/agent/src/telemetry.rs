//! Retained CPU, memory, network and SD measurements; no FPGA polling.
use serde_json::{Value, json};
use std::{
    ffi::CString,
    fs, mem,
    path::Path,
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CpuTimes {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
}

impl CpuTimes {
    fn total(self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }

    fn idle_total(self) -> u64 {
        self.idle.saturating_add(self.iowait)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct NetSample {
    rx_bytes: u64,
    tx_bytes: u64,
    at: Option<Instant>,
}

const SD_READ_BYTES_PER_SEC_AT_100_PCT: u64 = 50_000_000;
const SD_WRITE_BYTES_PER_SEC_AT_100_PCT: u64 = 25_000_000;
const DISK_SECTOR_BYTES: u64 = 512;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DiskCounters {
    sectors_read: u64,
    sectors_written: u64,
}

#[derive(Clone, Debug, Default)]
struct DiskSample {
    device: String,
    counters: DiskCounters,
    at: Option<Instant>,
}

fn parse_cpu_times_text(text: &str) -> Vec<CpuTimes> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let label = fields.next()?;
            if label != "cpu"
                && !label
                    .strip_prefix("cpu")?
                    .chars()
                    .all(|c| c.is_ascii_digit())
            {
                return None;
            }
            let nums = fields
                .take(8)
                .map(|field| field.parse::<u64>().unwrap_or(0))
                .collect::<Vec<_>>();
            Some(CpuTimes {
                user: *nums.first().unwrap_or(&0),
                nice: *nums.get(1).unwrap_or(&0),
                system: *nums.get(2).unwrap_or(&0),
                idle: *nums.get(3).unwrap_or(&0),
                iowait: *nums.get(4).unwrap_or(&0),
                irq: *nums.get(5).unwrap_or(&0),
                softirq: *nums.get(6).unwrap_or(&0),
                steal: *nums.get(7).unwrap_or(&0),
            })
        })
        .collect()
}

fn read_cpu_times() -> Option<Vec<CpuTimes>> {
    fs::read_to_string("/proc/stat")
        .ok()
        .map(|text| parse_cpu_times_text(&text))
}

fn cpu_busy_percent(previous: CpuTimes, current: CpuTimes) -> f64 {
    let total_delta = current.total().saturating_sub(previous.total());
    if total_delta == 0 {
        return 0.0;
    }
    let idle_delta = current.idle_total().saturating_sub(previous.idle_total());
    let busy = total_delta.saturating_sub(idle_delta);
    ((busy as f64 * 1000.0 / total_delta as f64).round()) / 10.0
}

fn cpu_json(previous: Option<&[CpuTimes]>, current: &[CpuTimes]) -> Value {
    let combined = match (previous.and_then(|items| items.first()), current.first()) {
        (Some(prev), Some(now)) => cpu_busy_percent(*prev, *now),
        _ => 0.0,
    };
    let cores = current
        .iter()
        .enumerate()
        .skip(1)
        .map(|(index, now)| {
            let busy_pct = previous
                .and_then(|items| items.get(index))
                .map(|prev| cpu_busy_percent(*prev, *now))
                .unwrap_or(0.0);
            json!({"id": index - 1, "busy_pct": busy_pct})
        })
        .collect::<Vec<_>>();
    json!({
        "combined_busy_pct": combined,
        "cores": cores,
    })
}

fn memory_split_json(
    mem_total_kb: u64,
    mem_available_kb: u64,
    magik_rss_kb: u64,
    main_rss_kb: u64,
) -> Value {
    let available_kb = mem_available_kb.min(mem_total_kb);
    let magik_kb = magik_rss_kb.min(mem_total_kb);
    let used_without_available = mem_total_kb.saturating_sub(available_kb);
    let other_used_kb = used_without_available.saturating_sub(magik_kb);
    json!({
        "total_kb": mem_total_kb,
        "available_kb": available_kb,
        "magik_kb": magik_kb,
        "main_kb": main_rss_kb,
        "other_used_kb": other_used_kb,
        "available_pct": percent_of(available_kb, mem_total_kb),
        "magik_pct": percent_of(magik_kb, mem_total_kb),
        "other_used_pct": percent_of(other_used_kb, mem_total_kb),
    })
}

fn memory_json(magik_rss_kb: u64, main_rss_kb: u64) -> Value {
    let meminfo = read_meminfo();
    let total = meminfo_value(&meminfo, "MemTotal").unwrap_or(0);
    let available = meminfo_value(&meminfo, "MemAvailable").unwrap_or_else(|| {
        meminfo_value(&meminfo, "MemFree").unwrap_or(0)
            + meminfo_value(&meminfo, "Buffers").unwrap_or(0)
            + meminfo_value(&meminfo, "Cached").unwrap_or(0)
    });
    memory_split_json(total, available, magik_rss_kb, main_rss_kb)
}

fn read_meminfo() -> Vec<(String, u64)> {
    fs::read_to_string("/proc/meminfo")
        .ok()
        .map(|text| {
            text.lines()
                .filter_map(|line| {
                    let (key, rest) = line.split_once(':')?;
                    let value = rest.split_whitespace().next()?.parse::<u64>().ok()?;
                    Some((key.to_string(), value))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn meminfo_value(items: &[(String, u64)], key: &str) -> Option<u64> {
    items
        .iter()
        .find_map(|(item_key, value)| (item_key == key).then_some(*value))
}

fn network_rate_json(previous: Option<NetSample>, current: NetSample) -> Value {
    let elapsed = previous
        .and_then(|previous| {
            Some(
                current
                    .at?
                    .saturating_duration_since(previous.at?)
                    .as_secs_f64(),
            )
        })
        .unwrap_or(0.0);
    let (rx_bps, tx_bps) = if elapsed > 0.0 {
        let previous = previous.unwrap_or_default();
        (
            ((current.rx_bytes.saturating_sub(previous.rx_bytes) as f64) / elapsed).round() as u64,
            ((current.tx_bytes.saturating_sub(previous.tx_bytes) as f64) / elapsed).round() as u64,
        )
    } else {
        (0, 0)
    };
    json!({
        "rx_bytes": current.rx_bytes,
        "tx_bytes": current.tx_bytes,
        "rx_bytes_per_sec": rx_bps,
        "tx_bytes_per_sec": tx_bps,
    })
}

fn network_json(previous: Option<NetSample>, fields: Option<[u64; 16]>, now: Instant) -> Value {
    match fields {
        Some(fields) => network_rate_json(
            previous,
            NetSample {
                rx_bytes: fields[0],
                tx_bytes: fields[8],
                at: Some(now),
            },
        ),
        None => Value::Null,
    }
}

fn parse_backing_disk(mounts: &str, diskstats: &str, path: &str) -> Option<String> {
    let source = mounts.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let source = fields.next()?;
        let mountpoint = fields.next()?;
        (mountpoint == path).then_some(source)
    });
    if let Some(source) = source {
        let base = source.rsplit('/').next().unwrap_or(source);
        if let Some(index) = base.rfind('p')
            && base.starts_with("mmcblk")
            && base[index + 1..].chars().all(|c| c.is_ascii_digit())
        {
            return Some(base[..index].to_string());
        }
        if base.starts_with("sd") {
            return Some(
                base.trim_end_matches(|c: char| c.is_ascii_digit())
                    .to_string(),
            );
        }
    }
    let candidates = diskstats
        .lines()
        .filter_map(|line| {
            let device = line.split_whitespace().nth(2)?;
            (device.starts_with("mmcblk")
                && !device.contains('p')
                && device[6..].chars().all(|c| c.is_ascii_digit()))
            .then(|| device.to_string())
        })
        .collect::<Vec<_>>();
    (candidates.len() == 1).then(|| candidates[0].clone())
}

fn backing_disk_for_path(path: &str) -> Option<String> {
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    let diskstats = fs::read_to_string("/proc/diskstats").unwrap_or_default();
    parse_backing_disk(&mounts, &diskstats, path)
}

fn parse_disk_counters(diskstats: &str, device: &str) -> Option<DiskCounters> {
    diskstats.lines().find_map(|line| {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.get(2).copied() != Some(device) {
            return None;
        }
        Some(DiskCounters {
            sectors_read: fields.get(5)?.parse().ok()?,
            sectors_written: fields.get(9)?.parse().ok()?,
        })
    })
}

fn read_disk_counters(device: &str) -> Option<DiskCounters> {
    parse_disk_counters(&fs::read_to_string("/proc/diskstats").ok()?, device)
}

fn disk_rate_bytes_per_sec(
    previous: DiskCounters,
    current: DiskCounters,
    elapsed: Duration,
) -> Option<(u64, u64)> {
    if elapsed.is_zero()
        || current.sectors_read < previous.sectors_read
        || current.sectors_written < previous.sectors_written
    {
        return None;
    }
    let seconds = elapsed.as_secs_f64();
    Some((
        ((current.sectors_read - previous.sectors_read) as f64 * DISK_SECTOR_BYTES as f64 / seconds)
            .round() as u64,
        ((current.sectors_written - previous.sectors_written) as f64 * DISK_SECTOR_BYTES as f64
            / seconds)
            .round() as u64,
    ))
}

fn throughput_percent(bytes_per_sec: u64, ceiling: u64) -> f64 {
    if ceiling == 0 {
        return 0.0;
    }
    (bytes_per_sec as f64 * 100.0 / ceiling as f64).clamp(0.0, 100.0)
}

fn disk_activity_json(
    previous: Option<&DiskSample>,
    device: Option<&str>,
    current: Option<DiskCounters>,
    now: Instant,
) -> Value {
    let rates = previous
        .zip(device)
        .zip(current)
        .filter(|((previous, device), _)| previous.device == *device)
        .and_then(|((previous, _), current)| {
            disk_rate_bytes_per_sec(
                previous.counters,
                current,
                now.saturating_duration_since(previous.at?),
            )
        });
    let valid = rates.is_some();
    let rates = rates.unwrap_or((0, 0));
    json!({
        "device": device.unwrap_or(""),
        "activity_valid": valid,
        "read_bytes_per_sec": rates.0,
        "write_bytes_per_sec": rates.1,
        "read_pct": throughput_percent(rates.0, SD_READ_BYTES_PER_SEC_AT_100_PCT),
        "write_pct": throughput_percent(rates.1, SD_WRITE_BYTES_PER_SEC_AT_100_PCT),
    })
}

fn storage_json(path: &str, activity: Value) -> Value {
    let Ok(c_path) = CString::new(path) else {
        return Value::Null;
    };
    let mut stats = mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: statvfs writes a valid statvfs struct when it returns 0; c_path is NUL-terminated.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) };
    if rc != 0 {
        return Value::Null;
    }
    // SAFETY: statvfs returned success, so stats is initialized.
    let stats = unsafe { stats.assume_init() };
    #[cfg(target_pointer_width = "64")]
    let block_size = stats.f_frsize;
    #[cfg(target_pointer_width = "32")]
    let block_size = u64::from(stats.f_frsize);
    let total_bytes = (stats.f_blocks as u64).saturating_mul(block_size);
    let available_bytes = (stats.f_bavail as u64).saturating_mul(block_size);
    let mut storage = json!({
        "path": path,
        "total_bytes": total_bytes,
        "available_bytes": available_bytes,
        "used_bytes": total_bytes.saturating_sub(available_bytes),
        "available_pct": percent_of(available_bytes, total_bytes),
    });
    if let (Some(storage), Some(activity)) = (storage.as_object_mut(), activity.as_object()) {
        storage.extend(activity.clone());
    }
    storage
}

fn percent_of(value: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        ((value as f64 * 1000.0 / total as f64).round()) / 10.0
    }
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

pub fn read_json(path: &str) -> Value {
    use std::io::Read;
    let Ok(file) = fs::File::open(path) else {
        return Value::Null;
    };
    let mut bytes = Vec::new();
    if file.take(512 * 1024 + 1).read_to_end(&mut bytes).is_err() || bytes.len() > 512 * 1024 {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

pub fn processes(root: &Path) -> Value {
    let mut result = json!({});
    let Ok(entries) = fs::read_dir(root) else {
        return Value::Null;
    };
    for name in ["mister-magik-fb", "MiSTer_MagiKDev", "MiSTer_MagiK"] {
        result[name] = json!({"pids":[], "rss_kb":0, "threads":0});
    }
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u64>() else {
            continue;
        };
        let Ok(comm) = fs::read_to_string(entry.path().join("comm")) else {
            continue;
        };
        let name = if comm.trim() == "magik"
            && fs::read_link(entry.path().join("exe"))
                .ok()
                .is_some_and(|path| {
                    path.to_string_lossy().trim_end_matches(" (deleted)")
                        == "/media/fat/mister-magik2/magik"
                }) {
            "mister-magik-fb"
        } else {
            comm.trim()
        };
        let Some(process) = result.get_mut(name) else {
            continue;
        };
        let Ok(status) = fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        process["pids"].as_array_mut().unwrap().push(json!(pid));
        for (source, target) in [("VmRSS:", "rss_kb"), ("Threads:", "threads")] {
            let value = status
                .lines()
                .find_map(|line| {
                    line.strip_prefix(source)?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
                .unwrap_or(0);
            process[target] = json!(process[target].as_u64().unwrap_or(0).saturating_add(value));
        }
    }
    result
}

pub fn current_status(processes: &Value) -> (Value, bool) {
    let status = read_json("/tmp/mister-magik/status.json");
    let current = status["pid"].as_u64().is_some_and(|pid| {
        processes["mister-magik-fb"]["pids"]
            .as_array()
            .is_some_and(|pids| pids.contains(&json!(pid)))
    });
    (status, current)
}

#[derive(Default)]
pub struct Sampler {
    cpu: Option<Vec<CpuTimes>>,
    net: Option<NetSample>,
    disk: Option<DiskSample>,
}
impl Sampler {
    pub fn sample(&mut self, seq: u64) -> Value {
        let now = Instant::now();
        let times = read_cpu_times();
        let cpu = match (self.cpu.as_deref(), times.as_deref()) {
            (Some(old), Some(new))
                if old.len() == new.len()
                    && old.iter().zip(new).all(|(a, b)| b.total() > a.total()) =>
            {
                cpu_json(Some(old), new)
            }
            _ => Value::Null,
        };
        self.cpu = times;
        let net = read_netdev_stats_fields("eth0");
        let network = match (self.net, net) {
            (Some(old), Some(fields)) if fields[0] >= old.rx_bytes && fields[8] >= old.tx_bytes => {
                network_json(Some(old), Some(fields), now)
            }
            _ => Value::Null,
        };
        self.net = net.map(|fields| NetSample {
            rx_bytes: fields[0],
            tx_bytes: fields[8],
            at: Some(now),
        });
        let device = backing_disk_for_path("/media/fat");
        let counters = device.as_deref().and_then(read_disk_counters);
        let activity = disk_activity_json(self.disk.as_ref(), device.as_deref(), counters, now);
        self.disk = device.zip(counters).map(|(device, counters)| DiskSample {
            device,
            counters,
            at: Some(now),
        });
        let processes = processes(Path::new("/proc"));
        let main = if processes["MiSTer_MagiKDev"]["pids"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
        {
            "MiSTer_MagiKDev"
        } else {
            "MiSTer_MagiK"
        };
        let memory = if Path::new("/proc/meminfo").exists() && !processes.is_null() {
            memory_json(
                processes["mister-magik-fb"]["rss_kb"].as_u64().unwrap_or(0),
                processes[main]["rss_kb"].as_u64().unwrap_or(0),
            )
        } else {
            Value::Null
        };
        let (status, current) = current_status(&processes);
        let mut launcher = json!({"status_current":current});
        if current {
            for key in [
                "idle",
                "rolling_fps",
                "fps_estimate",
                "preview_cache_state",
                "ui_thread_cpu",
            ] {
                launcher[key] = status[key].clone();
            }
            launcher["ui_thread_cpu"] = status["pid"]
                .as_u64()
                .and_then(|pid| fs::read_to_string(format!("/proc/{pid}/stat")).ok())
                .and_then(|text| {
                    text.rsplit_once(") ")?
                        .1
                        .split_whitespace()
                        .nth(36)?
                        .parse::<u64>()
                        .ok()
                })
                .map_or(Value::Null, |cpu| json!(cpu));
            let mut budget = json!({});
            for key in [
                "budget_us",
                "frames_total",
                "window_frames",
                "window_prepare_us",
                "window_render_us",
                "window_custom_draw_us",
                "window_vsync_us",
                "window_present_us",
            ] {
                budget[key] = status["frame_budget"][key].clone();
            }
            if let Some(frames) = status["frame_budget"]["recent_frames"].as_array() {
                budget["recent_frames"] = Value::Array(
                    frames
                        .iter()
                        .rev()
                        .take(120)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .map(|frame| {
                            let mut out = frame.clone();
                            if let Some(object) = out.as_object_mut() {
                                object.remove("vsync_miss_streak");
                            }
                            out
                        })
                        .collect(),
                );
            }
            launcher["frame_budget"] = budget;
        }
        json!({"seq":seq,"captured_unix_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,"cpu":cpu,"network":network,"memory":memory,"processes":processes,"storage":storage_json("/media/fat",activity),"launcher":launcher})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_application_is_included_in_process_analytics() {
        let root = std::env::temp_dir().join(format!("magik-process-alias-{}", std::process::id()));
        let process = root.join("42");
        fs::create_dir_all(&process).unwrap();
        fs::write(process.join("comm"), "magik\n").unwrap();
        fs::write(process.join("status"), "VmRSS: 100 kB\nThreads: 4\n").unwrap();
        std::os::unix::fs::symlink("/media/fat/mister-magik2/magik", process.join("exe")).unwrap();
        let result = processes(&root);
        assert_eq!(result["mister-magik-fb"]["pids"], json!([42]));
        assert_eq!(result["mister-magik-fb"]["rss_kb"], 100);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rates_preserve_measured_zero_and_reject_counter_reset() {
        let a = DiskCounters {
            sectors_read: 100,
            sectors_written: 20,
        };
        assert_eq!(
            disk_rate_bytes_per_sec(a, a, Duration::from_secs(1)),
            Some((0, 0))
        );
        assert_eq!(
            disk_rate_bytes_per_sec(a, DiskCounters::default(), Duration::from_secs(1)),
            None
        );
        let a = parse_cpu_times_text("cpu 10 0 0 90 0 0 0 0\n")[0];
        let b = parse_cpu_times_text("cpu 10 0 0 100 0 0 0 0\n")[0];
        assert_eq!(cpu_busy_percent(a, b), 0.0);
    }
}
