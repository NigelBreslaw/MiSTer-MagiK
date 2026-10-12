// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! CPU and SD-card evidence for fresh catalog builds in tooling builds.
//!
//! A fresh build records host-analysed files under
//! `/tmp/mister-magik2/profiles/catalog-fresh-build`, which the service serves
//! as profile artifacts. They live in tmpfs, so recording adds no SD-card I/O:
//! - `timeline.tsv`: CPU of the whole process and of the build thread (the
//!   rest of the process is the launcher's other threads), `/proc/self/io`,
//!   system CPU and SD-card diskstats every 50 ms;
//! - `tasks.tsv`: every 10 ms, each thread's state, kernel wait channel,
//!   current syscall and the path it names, which attributes blocked time;
//! - `stacks.tsv` and `maps.txt`: SIGPROF samples of ARM return addresses from
//!   the interrupted context and its `r11` frame chain, symbolised on the host
//!   from the unstripped executable;
//! - `marks.tsv`: builder milestones such as Arcade readiness.
//!
//! The syscall numbers and register names are 32-bit ARM Linux; on other
//! targets the module compiles but records no stacks or syscall detail.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const DIRECTORY: &str = "/tmp/mister-magik2/profiles/catalog-fresh-build";
const TASK_INTERVAL: Duration = Duration::from_millis(10);
const TIMELINE_EVERY: u32 = 5;
const PROFILE_INTERVAL_US: i64 = 10_000;
const STACK_DEPTH: usize = 40;
const STACK_WORDS: usize = STACK_DEPTH + 3;
const STACK_CAPACITY: usize = 20_000;
const TIMELINE_HEADER: &str = "t_ms\tutime\tstime\tbuild_utime\tbuild_stime\tread_bytes\twrite_bytes\tcpu_user\tcpu_nice\tcpu_system\tcpu_idle\tcpu_iowait\tcpu_irq\tcpu_softirq\tdisk_reads\tdisk_read_merges\tdisk_read_sectors\tdisk_read_ms\tdisk_writes\tdisk_write_merges\tdisk_write_sectors\tdisk_write_ms\tdisk_inflight\tdisk_io_ms\tdisk_weighted_ms";
const TASKS_HEADER: &str = "t_ms\ttid\tcomm\tstate\twchan\tsyscall\tdetail";

/// Sample records written from the signal handler, `STACK_WORDS` each:
/// `[tid, pc, lr, frame return addresses..., 0]`.
static STACK_BUFFER: OnceLock<&'static [AtomicU32]> = OnceLock::new();
static STACK_NEXT: AtomicUsize = AtomicUsize::new(0);
static STACK_DROPPED: AtomicU32 = AtomicU32::new(0);
static PROCESS_ID: AtomicI32 = AtomicI32::new(0);
static MARKS: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);

/// Record a builder milestone on the evidence clock; a no-op when idle.
pub fn mark(name: &str) {
    if let Ok(mut marks) = MARKS.lock()
        && let Some((started, rows)) = marks.as_mut()
    {
        rows.push(format!("{}\t{name}", started.elapsed().as_millis()));
    }
}

pub struct BuildEvidence {
    stop: Arc<AtomicBool>,
    sampler: Option<JoinHandle<(Vec<String>, Vec<String>)>>,
    disk: Option<String>,
    mount: Option<String>,
    profiler: Result<(), String>,
    started: Instant,
}

/// Start recording a fresh build, replacing the previous run's files.
/// Recording is best effort: failure is logged and never affects the build.
pub fn start_fresh_build() -> Option<BuildEvidence> {
    let result = fs::remove_dir_all(DIRECTORY)
        .or_else(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(error),
        })
        .and_then(|()| fs::create_dir_all(DIRECTORY))
        .map_err(|error| error.to_string())
        .and_then(|()| BuildEvidence::start());
    match result {
        Ok(evidence) => Some(evidence),
        Err(error) => {
            crate::ui_errln!("catalog build evidence unavailable: {error}");
            None
        }
    }
}

impl BuildEvidence {
    fn start() -> Result<Self, String> {
        let mount = fs::read_to_string("/proc/mounts").ok().and_then(|mounts| {
            mounts
                .lines()
                .find(|line| line.split_whitespace().nth(1) == Some("/media/fat"))
                .map(str::to_owned)
        });
        let disk = sd_disk(mount.as_deref());
        let stop = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        if let Ok(mut marks) = MARKS.lock() {
            *marks = Some((started, Vec::new()));
        }
        let sampler = {
            let stop = Arc::clone(&stop);
            let disk = disk.clone();
            // The build runs on the thread that starts recording.
            let build_tid = current_tid();
            std::thread::Builder::new()
                .name("build-evidence".to_owned())
                .spawn(move || sample_until(&stop, build_tid, disk.as_deref(), started))
                .map_err(|error| error.to_string())?
        };
        let profiler = start_profiler();
        Ok(Self {
            stop,
            sampler: Some(sampler),
            disk,
            mount,
            profiler,
            started,
        })
    }

    /// Stop recording and persist the run with the build's outcome.
    pub fn finish(mut self, outcome: &str) {
        if let Err(error) = self.persist(outcome) {
            crate::ui_errln!("catalog build evidence failed: {error}");
        }
    }

    fn persist(&mut self, outcome: &str) -> Result<(), String> {
        let wall_ms = self.started.elapsed().as_millis() as u64;
        if self.profiler.is_ok() {
            stop_profiler();
        }
        let maps = fs::read_to_string("/proc/self/maps").unwrap_or_default();
        self.stop.store(true, Ordering::Release);
        let (timeline, tasks) = self
            .sampler
            .take()
            .ok_or("evidence already persisted")?
            .join()
            .map_err(|_| "evidence sampler panicked")?;
        let marks = MARKS
            .lock()
            .ok()
            .and_then(|mut marks| marks.take())
            .map(|(_, rows)| rows)
            .unwrap_or_default();
        let stacks = drain_stacks();
        let directory = Path::new(DIRECTORY);
        write_rows(directory, "timeline.tsv", TIMELINE_HEADER, &timeline)?;
        write_rows(directory, "tasks.tsv", TASKS_HEADER, &tasks)?;
        write_rows(directory, "marks.tsv", "t_ms\tname", &marks)?;
        write_rows(directory, "stacks.tsv", "tid\tpc\tlr\tframes", &stacks)?;
        fs::write(directory.join("maps.txt"), maps).map_err(|error| error.to_string())?;
        let evidence = serde_json::json!({
            "schema":"mister-magik-catalog-build-evidence-v1",
            "outcome":outcome,
            "kernel":fs::read_to_string("/proc/version").unwrap_or_default().trim(),
            "executable":fs::read_link("/proc/self/exe").ok().map(|path| path.display().to_string()),
            "wall_ms":wall_ms,
            "clock_ticks_per_second":clock_ticks(),
            "timeline_interval_ms":(TASK_INTERVAL * TIMELINE_EVERY).as_millis() as u64,
            "task_interval_ms":TASK_INTERVAL.as_millis() as u64,
            "profile_interval_us":PROFILE_INTERVAL_US,
            "timeline_samples":timeline.len(),
            "task_samples":tasks.len(),
            "stack_samples":stacks.len(),
            "stack_samples_dropped":STACK_DROPPED.load(Ordering::Relaxed),
            "sd_disk":self.disk,
            "sd_mount":self.mount,
            "sd_queue":sd_queue(self.disk.as_deref()),
            "profiler_error":self.profiler.as_ref().err(),
        });
        // Written last: the host treats its presence as "run complete".
        fs::write(
            directory.join("evidence.json"),
            serde_json::to_vec_pretty(&evidence).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())
    }
}

impl Drop for BuildEvidence {
    /// A recorder dropped without `finish` must not keep sampling the launcher.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if self.profiler.is_ok() {
            stop_profiler();
        }
    }
}

fn write_rows(directory: &Path, name: &str, header: &str, rows: &[String]) -> Result<(), String> {
    let mut text = String::with_capacity(rows.iter().map(|row| row.len() + 1).sum::<usize>() + 128);
    text.push_str(header);
    text.push('\n');
    for row in rows {
        text.push_str(row);
        text.push('\n');
    }
    fs::write(directory.join(name), text).map_err(|error| error.to_string())
}

fn sample_until(
    stop: &AtomicBool,
    build_tid: i32,
    disk: Option<&str>,
    started: Instant,
) -> (Vec<String>, Vec<String>) {
    let own = current_tid();
    let mut timeline = Vec::new();
    let mut tasks = Vec::new();
    let mut next = started;
    let mut tick = 0u32;
    loop {
        let t_ms = started.elapsed().as_millis();
        sample_tasks(t_ms, own, &mut tasks);
        let stopping = stop.load(Ordering::Acquire);
        if tick.is_multiple_of(TIMELINE_EVERY) || stopping {
            timeline.push(sample(t_ms, build_tid, disk));
        }
        if stopping {
            return (timeline, tasks);
        }
        tick = tick.wrapping_add(1);
        next += TASK_INTERVAL;
        if let Some(delay) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(delay);
        }
    }
}

fn sample_tasks(t_ms: u128, own: i32, rows: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir("/proc/self/task") else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let tid = entry.file_name();
        let tid = tid.to_string_lossy();
        if tid.parse::<i32>().ok() == Some(own) {
            continue;
        }
        let path = entry.path();
        let Ok(stat) = fs::read_to_string(path.join("stat")) else {
            continue;
        };
        let Some((head, rest)) = stat.rsplit_once(')') else {
            continue;
        };
        let comm = head.split_once('(').map_or("", |(_, name)| name);
        let state = rest.split_whitespace().next().unwrap_or("?");
        // Running threads are attributed by the CPU sampler; sleeping and
        // blocked threads record the kernel wait and the syscall's target.
        let (wchan, syscall, detail) = if state == "R" {
            (String::new(), String::new(), String::new())
        } else {
            let text = fs::read_to_string(path.join("syscall")).unwrap_or_default();
            let fields: Vec<&str> = text.split_whitespace().collect();
            let syscall = fields.first().copied().unwrap_or("").to_owned();
            let argument = |index: usize| {
                fields.get(index + 1).and_then(|value| {
                    usize::from_str_radix(value.trim_start_matches("0x"), 16).ok()
                })
            };
            (
                fs::read_to_string(path.join("wchan")).unwrap_or_default(),
                syscall.clone(),
                syscall_detail(&syscall, argument),
            )
        };
        rows.push(format!(
            "{t_ms}\t{tid}\t{comm}\t{state}\t{wchan}\t{syscall}\t{detail}"
        ));
    }
}

/// The path a blocked thread's syscall names, from its path or fd argument.
fn syscall_detail(syscall: &str, argument: impl Fn(usize) -> Option<usize>) -> String {
    // 32-bit ARM EABI syscall numbers.
    let path_argument = match syscall {
        "5" | "10" | "38" | "39" | "40" | "195" | "196" => Some(0),
        "322" | "327" | "328" | "397" => Some(1),
        _ => None,
    };
    if let Some(index) = path_argument {
        let name = argument(index).map(read_string).unwrap_or_default();
        // `*at` calls name a path relative to the directory fd in argument 0.
        if index == 1 && !name.starts_with('/') {
            let directory = argument(0)
                .filter(|&fd| fd as i32 != libc::AT_FDCWD)
                .and_then(|fd| fs::read_link(format!("/proc/self/fd/{fd}")).ok());
            if let Some(directory) = directory {
                return format!("{}/{name}", directory.display());
            }
        }
        return name;
    }
    let fd_argument = matches!(
        syscall,
        "3" | "4" | "6" | "19" | "118" | "140" | "148" | "180" | "181" | "197" | "217" | "362"
    );
    if fd_argument {
        return argument(0)
            .and_then(|fd| fs::read_link(format!("/proc/self/fd/{fd}")).ok())
            .map(|path| path.display().to_string())
            .unwrap_or_default();
    }
    String::new()
}

/// Copy bytes from this process's address space. `process_vm_readv` reports a
/// stale or garbage pointer as an error instead of faulting, which also makes
/// it safe inside the profiling signal handler.
#[cfg(target_os = "linux")]
fn read_memory(address: usize, buffer: &mut [u8]) -> usize {
    let local = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    let remote = libc::iovec {
        iov_base: address as *mut libc::c_void,
        iov_len: buffer.len(),
    };
    // SAFETY: the kernel validates the remote range and writes only `buffer`.
    let read = unsafe {
        libc::process_vm_readv(PROCESS_ID.load(Ordering::Relaxed), &local, 1, &remote, 1, 0)
    };
    usize::try_from(read).unwrap_or(0)
}

#[cfg(not(target_os = "linux"))]
fn read_memory(_: usize, _: &mut [u8]) -> usize {
    0
}

fn read_string(address: usize) -> String {
    let mut buffer = [0u8; 256];
    let read = read_memory(address, &mut buffer);
    let bytes = &buffer[..read];
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).replace(['\t', '\n'], " ")
}

fn sample(t_ms: u128, build_tid: i32, disk: Option<&str>) -> String {
    let process = proc_stat_fields("/proc/self/stat");
    let build = proc_stat_fields(&format!("/proc/self/task/{build_tid}/stat"));
    let field = |fields: &[String], number: usize| {
        fields
            .get(number - 3)
            .cloned()
            .unwrap_or_else(|| "0".into())
    };
    let io = fs::read_to_string("/proc/self/io").unwrap_or_default();
    let io_field = |key: &str| {
        io.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(": "))
            .unwrap_or("0")
            .trim()
            .to_owned()
    };
    let system = fs::read_to_string("/proc/stat").unwrap_or_default();
    let cpu: Vec<&str> = system
        .lines()
        .next()
        .filter(|line| line.starts_with("cpu "))
        .map(|line| line.split_whitespace().skip(1).take(7).collect())
        .unwrap_or_default();
    let disk_fields: Vec<String> = disk
        .and_then(|name| {
            fs::read_to_string("/proc/diskstats")
                .ok()?
                .lines()
                .map(|line| line.split_whitespace().collect::<Vec<_>>())
                .find(|parts| parts.get(2) == Some(&name))
                .map(|parts| {
                    parts
                        .iter()
                        .skip(3)
                        .take(11)
                        .map(|s| (*s).to_owned())
                        .collect()
                })
        })
        .unwrap_or_default();
    let mut row = vec![
        t_ms.to_string(),
        field(&process, 14),
        field(&process, 15),
        field(&build, 14),
        field(&build, 15),
        io_field("read_bytes"),
        io_field("write_bytes"),
    ];
    row.extend((0..7).map(|i| cpu.get(i).copied().unwrap_or("0").to_owned()));
    row.extend((0..11).map(|i| disk_fields.get(i).cloned().unwrap_or_else(|| "0".into())));
    row.join("\t")
}

/// Fields after the parenthesised command: index 0 is `/proc/PID/stat` field 3.
fn proc_stat_fields(path: &str) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn sd_disk(mount: Option<&str>) -> Option<String> {
    let diskstats = fs::read_to_string("/proc/diskstats").ok()?;
    let names: Vec<&str> = diskstats
        .lines()
        .filter_map(|line| line.split_whitespace().nth(2))
        .collect();
    choose_sd_disk(&names, mount)
}

/// The SD-card block device: the whole disk behind the `/media/fat` mount
/// source when it is an `mmcblk` device, otherwise the first whole `mmcblk`.
fn choose_sd_disk(names: &[&str], mount: Option<&str>) -> Option<String> {
    let source = mount
        .and_then(|line| line.split_whitespace().next())
        .map(|source| source.trim_start_matches("/dev/"));
    if let Some(source) = source
        && source.starts_with("mmcblk")
    {
        let disk = source.split_once('p').map_or(source, |(disk, _)| disk);
        if names.contains(&disk) {
            return Some(disk.to_owned());
        }
    }
    names
        .iter()
        .find(|name| name.starts_with("mmcblk") && !name.contains('p'))
        .map(|name| (*name).to_owned())
}

fn sd_queue(disk: Option<&str>) -> serde_json::Value {
    let Some(disk) = disk else {
        return serde_json::Value::Null;
    };
    let read = |name: &str| {
        fs::read_to_string(format!("/sys/block/{disk}/queue/{name}"))
            .map(|value| value.trim().to_owned())
            .unwrap_or_default()
    };
    serde_json::json!({
        "scheduler":read("scheduler"),
        "read_ahead_kb":read("read_ahead_kb"),
        "max_sectors_kb":read("max_sectors_kb"),
        "nr_requests":read("nr_requests"),
    })
}

fn clock_ticks() -> i64 {
    // SAFETY: sysconf has no preconditions.
    // `c_long` is 32-bit on the device and 64-bit on development hosts.
    #[allow(clippy::useless_conversion)]
    i64::from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) })
}

#[cfg(target_os = "linux")]
fn current_tid() -> i32 {
    // SAFETY: gettid has no preconditions.
    unsafe { libc::syscall(libc::SYS_gettid) as i32 }
}

#[cfg(not(target_os = "linux"))]
fn current_tid() -> i32 {
    0
}

fn start_profiler() -> Result<(), String> {
    PROCESS_ID.store(std::process::id() as i32, Ordering::Relaxed);
    // SAFETY: the action structs are zeroed before use; the handler is
    // async-signal-safe: atomics, `gettid` and `process_vm_readv`.
    unsafe {
        // The tooling renderer profile (pprof) owns SIGPROF when it is active.
        let mut previous: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(libc::SIGPROF, std::ptr::null(), &mut previous) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if previous.sa_sigaction != libc::SIG_DFL {
            return Err("SIGPROF is already in use".into());
        }
        // One buffer for the process: the handler may still be running when the
        // timer is disarmed, so it is never freed, and reused by later builds.
        STACK_BUFFER.get_or_init(|| {
            Box::leak(
                (0..STACK_CAPACITY * STACK_WORDS)
                    .map(|_| AtomicU32::new(0))
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            )
        });
        STACK_NEXT.store(0, Ordering::Relaxed);
        STACK_DROPPED.store(0, Ordering::Relaxed);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_profile_signal
            as extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void)
            as usize;
        action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        if libc::sigaction(libc::SIGPROF, &action, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    set_profile_timer(PROFILE_INTERVAL_US)
}

/// Disarm the timer and restore the default handler. Only call this after
/// `start_profiler` succeeded, so another owner's SIGPROF setup is untouched.
fn stop_profiler() {
    let _ = set_profile_timer(0);
    // SAFETY: restoring the default disposition has no preconditions.
    unsafe { libc::signal(libc::SIGPROF, libc::SIG_DFL) };
}

fn set_profile_timer(interval_us: i64) -> Result<(), String> {
    let interval = libc::timeval {
        tv_sec: 0,
        tv_usec: interval_us as libc::suseconds_t,
    };
    let timer = libc::itimerval {
        it_interval: interval,
        it_value: interval,
    };
    // SAFETY: the timer struct is fully initialised.
    if unsafe { libc::setitimer(libc::ITIMER_PROF, &timer, std::ptr::null_mut()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

/// Program counter, link register, frame pointer and stack pointer.
#[cfg(all(target_arch = "arm", target_os = "linux"))]
fn interrupted_registers(context: *mut libc::c_void) -> Option<[usize; 4]> {
    if context.is_null() {
        return None;
    }
    // SAFETY: SIGPROF supplies the interrupted thread's ucontext.
    let registers = unsafe { &(*(context as *const libc::ucontext_t)).uc_mcontext };
    Some([
        registers.arm_pc as usize,
        registers.arm_lr as usize,
        registers.arm_fp as usize,
        registers.arm_sp as usize,
    ])
}

#[cfg(not(all(target_arch = "arm", target_os = "linux")))]
fn interrupted_registers(_: *mut libc::c_void) -> Option<[usize; 4]> {
    None
}

extern "C" fn on_profile_signal(
    _: libc::c_int,
    _: *mut libc::siginfo_t,
    context: *mut libc::c_void,
) {
    let Some(buffer) = STACK_BUFFER.get() else {
        return;
    };
    let Some([pc, lr, mut fp, sp]) = interrupted_registers(context) else {
        return;
    };
    let slot = STACK_NEXT.fetch_add(1, Ordering::Relaxed);
    if slot >= STACK_CAPACITY {
        STACK_DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let record = &buffer[slot * STACK_WORDS..][..STACK_WORDS];
    record[0].store(current_tid() as u32, Ordering::Relaxed);
    record[1].store(pc as u32, Ordering::Relaxed);
    record[2].store(lr as u32, Ordering::Relaxed);
    // LLVM's ARM frame record is `[saved r11, lr]` at r11. Follow only
    // aligned, strictly ascending records above the stack pointer.
    let mut depth = 0;
    while depth < STACK_DEPTH && fp.is_multiple_of(4) && fp >= sp {
        let mut frame = [0u8; 8];
        if read_memory(fp, &mut frame) != frame.len() {
            break;
        }
        let previous = u32::from_ne_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        let ret = u32::from_ne_bytes([frame[4], frame[5], frame[6], frame[7]]);
        if ret == 0 {
            break;
        }
        record[3 + depth].store(ret, Ordering::Relaxed);
        depth += 1;
        if previous <= fp {
            break;
        }
        fp = previous;
    }
    if depth < STACK_DEPTH {
        record[3 + depth].store(0, Ordering::Relaxed);
    }
}

fn drain_stacks() -> Vec<String> {
    let Some(buffer) = STACK_BUFFER.get() else {
        return Vec::new();
    };
    let count = STACK_NEXT.load(Ordering::Acquire).min(STACK_CAPACITY);
    (0..count)
        .map(|slot| {
            let record = &buffer[slot * STACK_WORDS..][..STACK_WORDS];
            let word = |index: usize| record[index].load(Ordering::Relaxed);
            let frames: Vec<String> = (3..STACK_WORDS)
                .map(word)
                .take_while(|&address| address != 0)
                .map(|address| format!("{address:x}"))
                .collect();
            format!(
                "{}\t{:x}\t{:x}\t{}",
                word(0),
                word(1),
                word(2),
                frames.join(",")
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syscall_detail_ignores_unknown_syscalls_and_unreadable_pointers() {
        assert_eq!(syscall_detail("240", |_| Some(0x1000)), "");
        assert_eq!(syscall_detail("5", |_| None), "");
        assert_eq!(syscall_detail("3", |_| None), "");
    }

    #[test]
    fn proc_stat_fields_skip_a_command_name_containing_parentheses() {
        let path = std::env::temp_dir().join(format!("evidence-stat-{}", std::process::id()));
        fs::write(&path, "42 (a (b) c) S 1 2 3").unwrap();
        let fields = proc_stat_fields(path.to_str().unwrap());
        fs::remove_file(&path).unwrap();
        assert_eq!(fields, ["S", "1", "2", "3"]);
    }

    #[test]
    fn sd_disk_is_the_whole_device_behind_the_fat_mount() {
        let names = ["loop0", "mmcblk0", "mmcblk0p1", "mmcblk0p2", "mmcblk1"];
        assert_eq!(
            choose_sd_disk(&names, Some("/dev/mmcblk1p1 /media/fat exfat rw 0 0")).as_deref(),
            Some("mmcblk1")
        );
        // `/dev/root` hides the device, so fall back to the first whole card.
        assert_eq!(
            choose_sd_disk(&names, Some("/dev/root /media/fat exfat rw 0 0")).as_deref(),
            Some("mmcblk0")
        );
        assert_eq!(choose_sd_disk(&["sda"], None), None);
    }

    #[test]
    fn marks_are_ignored_until_a_build_starts() {
        *MARKS.lock().unwrap() = None;
        mark("ignored");
        assert!(MARKS.lock().unwrap().is_none());
    }
}
