//! System-wide scheduling evidence for a measurement window: which threads
//! and interrupts held each CPU. Snapshots are taken only at window
//! boundaries; parsing is separate from procfs so it is testable anywhere.
use serde_json::{Value, json};
use std::collections::HashMap;

/// Threads reported per window, ordered by CPU time used.
const REPORTED_THREADS: usize = 24;
/// Interrupt lines reported per window, ordered by count.
const REPORTED_INTERRUPTS: usize = 12;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub idle: u64,
    pub irq: u64,
    pub softirq: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadSample {
    pub process: String,
    /// Parent process id and command line, to trace short-lived processes.
    pub parent: Option<u32>,
    pub command: String,
    pub thread: String,
    pub runtime_ns: u64,
    pub run_delay_ns: u64,
    pub timeslices: u64,
    pub last_cpu: Option<u32>,
    pub nice: Option<i64>,
    pub policy: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Per CPU, in USER_HZ ticks.
    pub cpus: Vec<CpuTimes>,
    /// Interrupt line label and description to per-CPU counts.
    pub interrupts: HashMap<String, (String, Vec<u64>)>,
    /// Softirq name to per-CPU counts.
    pub softirqs: HashMap<String, Vec<u64>>,
    pub threads: HashMap<(u32, u32), ThreadSample>,
}

/// Per-CPU lines (`cpuN`) of /proc/stat.
pub fn parse_stat(text: &str) -> Vec<CpuTimes> {
    text.lines()
        .filter(|line| line.starts_with("cpu") && !line.starts_with("cpu "))
        .filter_map(|line| {
            let v: Vec<u64> = line
                .split_whitespace()
                .skip(1)
                .filter_map(|field| field.parse().ok())
                .collect();
            // user nice system idle iowait irq softirq steal
            (v.len() >= 7).then(|| CpuTimes {
                busy: v[0] + v[1] + v[2] + v.get(7).copied().unwrap_or(0),
                idle: v[3] + v[4],
                irq: v[5],
                softirq: v[6],
            })
        })
        .collect()
}

/// /proc/interrupts or /proc/softirqs: a CPU header, then `label: counts... description`.
pub fn parse_per_cpu_table(text: &str) -> HashMap<String, (String, Vec<u64>)> {
    let mut lines = text.lines();
    let cpus = lines
        .next()
        .map_or(0, |header| header.split_whitespace().count());
    lines
        .filter_map(|line| {
            let (label, rest) = line.split_once(':')?;
            let mut fields = rest.split_whitespace();
            let counts: Vec<u64> = fields
                .by_ref()
                .take(cpus)
                .map_while(|field| field.parse().ok())
                .collect();
            (counts.len() == cpus).then(|| {
                let description = fields.collect::<Vec<_>>().join(" ");
                (label.trim().to_owned(), (description, counts))
            })
        })
        .collect()
}

/// `/proc/<pid>/task/<tid>/stat` fields after the parenthesised command.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_task_stat(text: &str) -> (Option<u32>, Option<i64>, Option<u32>) {
    let Some((_, after)) = text.rsplit_once(')') else {
        return (None, None, None);
    };
    // Field 3 (state) is the first after the command; field N is index N - 3.
    let fields: Vec<&str> = after.split_whitespace().collect();
    let at = |field: usize| fields.get(field - 3).copied();
    (
        at(39).and_then(|v| v.parse().ok()),
        at(19).and_then(|v| v.parse().ok()),
        at(41).and_then(|v| v.parse().ok()),
    )
}

/// Parent pid (field 4) of `/proc/<pid>/stat`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_parent(text: &str) -> Option<u32> {
    text.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_schedstat(text: &str) -> Option<(u64, u64, u64)> {
    let mut fields = text.split_whitespace().map(|v| v.parse::<u64>().ok());
    Some((fields.next()??, fields.next()??, fields.next()??))
}

#[cfg(target_os = "linux")]
pub fn snapshot() -> Snapshot {
    use std::fs::{read_dir, read_to_string};
    let read = |path: &str| read_to_string(path).unwrap_or_default();
    let mut threads = HashMap::new();
    for process in read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = process
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let process_name = read(&format!("/proc/{pid}/comm")).trim().to_owned();
        let command = read(&format!("/proc/{pid}/cmdline"))
            .split('\0')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect::<String>();
        let parent = parse_parent(&read(&format!("/proc/{pid}/stat")));
        let tasks = read_dir(format!("/proc/{pid}/task"))
            .into_iter()
            .flatten()
            .flatten();
        for task in tasks {
            let Some(tid) = task
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            let base = format!("/proc/{pid}/task/{tid}");
            let Some((runtime_ns, run_delay_ns, timeslices)) =
                parse_schedstat(&read(&format!("{base}/schedstat")))
            else {
                continue;
            };
            let (last_cpu, nice, policy) = parse_task_stat(&read(&format!("{base}/stat")));
            threads.insert(
                (pid, tid),
                ThreadSample {
                    process: process_name.clone(),
                    parent,
                    command: command.clone(),
                    thread: read(&format!("{base}/comm")).trim().to_owned(),
                    runtime_ns,
                    run_delay_ns,
                    timeslices,
                    last_cpu,
                    nice,
                    policy,
                },
            );
        }
    }
    Snapshot {
        cpus: parse_stat(&read("/proc/stat")),
        interrupts: parse_per_cpu_table(&read("/proc/interrupts")),
        softirqs: parse_per_cpu_table(&read("/proc/softirqs"))
            .into_iter()
            .map(|(name, (_, counts))| (name, counts))
            .collect(),
        threads,
    }
}

#[cfg(not(target_os = "linux"))]
pub fn snapshot() -> Snapshot {
    Snapshot::default()
}

/// A thread's identity, last sample, and runtime, run delay and timeslices
/// used during the window.
type ThreadUse<'a> = (&'a (u32, u32), &'a ThreadSample, u64, u64, u64);

fn delta(after: &[u64], before: Option<&Vec<u64>>) -> Vec<u64> {
    after
        .iter()
        .enumerate()
        .map(|(cpu, value)| {
            value.saturating_sub(before.and_then(|b| b.get(cpu)).copied().unwrap_or(0))
        })
        .collect()
}

/// What ran on each CPU between two snapshots. Threads that exited during the
/// window are absent; threads that started count from zero.
pub fn report(before: &Snapshot, after: &Snapshot, ticks_per_second: u64) -> Value {
    let tick_ms = 1000.0 / ticks_per_second.max(1) as f64;
    let cpus: Vec<Value> = after
        .cpus
        .iter()
        .enumerate()
        .map(|(cpu, end)| {
            let start = before.cpus.get(cpu).cloned().unwrap_or_default();
            let ms = |end: u64, start: u64| end.saturating_sub(start) as f64 * tick_ms;
            json!({
                "cpu": cpu,
                "busy_ms": ms(end.busy, start.busy),
                "irq_ms": ms(end.irq, start.irq),
                "softirq_ms": ms(end.softirq, start.softirq),
                "idle_ms": ms(end.idle, start.idle),
            })
        })
        .collect();
    let mut interrupts: Vec<(String, String, Vec<u64>)> = after
        .interrupts
        .iter()
        .map(|(label, (description, counts))| {
            let previous = before.interrupts.get(label).map(|(_, counts)| counts);
            (label.clone(), description.clone(), delta(counts, previous))
        })
        .filter(|(_, _, counts)| counts.iter().any(|&count| count > 0))
        .collect();
    interrupts.sort_by_key(|(_, _, counts)| std::cmp::Reverse(counts.iter().sum::<u64>()));
    let mut softirqs: Vec<(String, Vec<u64>)> = after
        .softirqs
        .iter()
        .map(|(name, counts)| (name.clone(), delta(counts, before.softirqs.get(name))))
        .filter(|(_, counts)| counts.iter().any(|&count| count > 0))
        .collect();
    softirqs.sort_by_key(|(_, counts)| std::cmp::Reverse(counts.iter().sum::<u64>()));
    let mut threads: Vec<ThreadUse<'_>> = after
        .threads
        .iter()
        .map(|(id, end)| {
            let start = before.threads.get(id);
            let field = |f: fn(&ThreadSample) -> u64| f(end).saturating_sub(start.map_or(0, f));
            (
                id,
                end,
                field(|t| t.runtime_ns),
                field(|t| t.run_delay_ns),
                field(|t| t.timeslices),
            )
        })
        .filter(|(_, _, runtime, _, _)| *runtime > 0)
        .collect();
    threads.sort_by_key(|(_, _, runtime, _, _)| std::cmp::Reverse(*runtime));
    json!({
        "cpus": cpus,
        "interrupts": interrupts
            .into_iter()
            .take(REPORTED_INTERRUPTS)
            .map(|(label, description, counts)| json!({
                "irq": label, "description": description, "per_cpu": counts,
            }))
            .collect::<Vec<_>>(),
        "softirqs": softirqs
            .into_iter()
            .map(|(name, counts)| json!({"name": name, "per_cpu": counts}))
            .collect::<Vec<_>>(),
        "threads": threads
            .into_iter()
            .take(REPORTED_THREADS)
            .map(|((pid, tid), sample, runtime, run_delay, timeslices)| json!({
                "pid": pid,
                "tid": tid,
                "process": sample.process,
                "parent": sample.parent,
                "parent_process": sample.parent.and_then(|parent| {
                    after.threads.get(&(parent, parent)).map(|p| p.process.clone())
                }),
                "command": sample.command,
                "thread": sample.thread,
                "cpu_ms": runtime as f64 / 1e6,
                "run_delay_ms": run_delay as f64 / 1e6,
                "timeslices": timeslices,
                "last_cpu": sample.last_cpu,
                "nice": sample.nice,
                "policy": sample.policy,
            }))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_interrupt_and_task_tables() {
        let stat = "cpu  10 0 5 100 0 1 2 0 0 0\ncpu0 6 0 3 40 1 1 2 0 0 0\ncpu1 4 0 2 60 0 0 0 0 0 0\nintr 1\n";
        assert_eq!(
            parse_stat(stat),
            vec![
                CpuTimes {
                    busy: 9,
                    idle: 41,
                    irq: 1,
                    softirq: 2
                },
                CpuTimes {
                    busy: 6,
                    idle: 60,
                    irq: 0,
                    softirq: 0
                },
            ]
        );
        let interrupts = "           CPU0       CPU1       \n 24:      12345          0     GIC-0  29 Level     twd\nIPI0:          0          7  CPU wakeup interrupts\nErr:          0\n";
        let table = parse_per_cpu_table(interrupts);
        assert_eq!(
            table["24"],
            ("GIC-0 29 Level twd".to_owned(), vec![12345, 0])
        );
        assert_eq!(table["IPI0"].1, vec![0, 7]);
        assert!(!table.contains_key("Err"));
        // Fields 21-38 are zero; processor (39) is 1, rt_priority and policy are 0.
        let task = "123 (card tile) helper) R 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 -5 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 0";
        assert_eq!(parse_task_stat(task), (Some(1), Some(-5), Some(0)));
        assert_eq!(parse_schedstat("1000 2000 3\n"), Some((1000, 2000, 3)));
        assert_eq!(parse_parent("4312 (sh) S 889 4312 889"), Some(889));
    }

    #[test]
    fn report_ranks_threads_and_interrupts_by_window_use() {
        let thread = |runtime_ns, run_delay_ns, last_cpu| ThreadSample {
            process: "magik".into(),
            parent: None,
            command: String::new(),
            thread: "card-tile-helper".into(),
            runtime_ns,
            run_delay_ns,
            timeslices: 1,
            last_cpu: Some(last_cpu),
            nice: Some(-5),
            policy: Some(0),
        };
        let before = Snapshot {
            cpus: vec![CpuTimes {
                busy: 100,
                idle: 100,
                irq: 1,
                softirq: 1,
            }],
            interrupts: HashMap::from([("24".into(), ("twd".into(), vec![10]))]),
            softirqs: HashMap::from([("TIMER".into(), vec![5])]),
            threads: HashMap::from([((1, 1), thread(1_000_000, 0, 0))]),
        };
        let after = Snapshot {
            cpus: vec![CpuTimes {
                busy: 150,
                idle: 150,
                irq: 3,
                softirq: 6,
            }],
            interrupts: HashMap::from([
                ("24".into(), ("twd".into(), vec![110])),
                ("40".into(), ("usb".into(), vec![500])),
            ]),
            softirqs: HashMap::from([("TIMER".into(), vec![9])]),
            threads: HashMap::from([
                ((1, 1), thread(5_000_000, 2_000_000, 0)),
                ((2, 2), thread(9_000_000, 0, 1)),
            ]),
        };
        let report = report(&before, &after, 100);
        assert_eq!(report["cpus"][0]["irq_ms"], 20.0);
        assert_eq!(report["cpus"][0]["softirq_ms"], 50.0);
        assert_eq!(report["interrupts"][0]["description"], "usb");
        assert_eq!(report["interrupts"][1]["per_cpu"][0], 100);
        assert_eq!(report["softirqs"][0]["per_cpu"][0], 4);
        assert_eq!(report["threads"][0]["tid"], 2);
        assert_eq!(report["threads"][1]["cpu_ms"], 4.0);
        assert_eq!(report["threads"][1]["run_delay_ms"], 2.0);
    }
}
