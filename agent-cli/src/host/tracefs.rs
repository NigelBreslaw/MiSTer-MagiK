// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::collections::{BTreeMap, HashMap};

const TRACEFS_MOUNT: &str = "/sys/kernel/tracing";
const TRACE_BUFFER_KB: u64 = 4_096;
const TRACE_MAX_BYTES: u64 = 32 * 1024 * 1024;

pub(super) const SCHEDULER_TRACE_SPEC: TracefsCaptureSpec = TracefsCaptureSpec {
    label: "scheduler trace",
    instance: "mister-magik-scheduler",
    remote_root: "/tmp/mister-magik/scheduler-trace",
    required_events: &[
        "sched:sched_switch",
        "sched:sched_wakeup",
        "sched:sched_migrate_task",
        "irq:irq_handler_entry",
        "irq:irq_handler_exit",
        "irq:softirq_entry",
        "irq:softirq_exit",
    ],
    optional_events: &[
        "sched:sched_waking",
        "sched:sched_process_fork",
        "sched:sched_process_exit",
        "workqueue:workqueue_execute_start",
        "workqueue:workqueue_execute_end",
        "block:block_rq_issue",
        "block:block_rq_complete",
    ],
};

#[derive(Clone, Copy)]
pub(super) struct TracefsCaptureSpec {
    pub(super) label: &'static str,
    pub(super) instance: &'static str,
    pub(super) remote_root: &'static str,
    pub(super) required_events: &'static [&'static str],
    pub(super) optional_events: &'static [&'static str],
}

pub(super) struct TracefsCapture<'a> {
    session: &'a Session,
    output_dir: &'a Path,
    spec: TracefsCaptureSpec,
}

pub(super) struct RetainedTrace {
    pub(super) raw_path: PathBuf,
    pub(super) stats: String,
    pub(super) capabilities: String,
    pub(super) sha256: String,
}

impl<'a> TracefsCapture<'a> {
    pub(super) fn new(
        session: &'a Session,
        output_dir: &'a Path,
        spec: TracefsCaptureSpec,
    ) -> Self {
        Self {
            session,
            output_dir,
            spec,
        }
    }

    pub(super) fn prepare(&self) -> Result<()> {
        fs::create_dir_all(self.output_dir)?;
        exec_checked(
            self.session,
            &format!("clean stale {} state", self.spec.label),
            &tracefs_cleanup_command(self.spec),
        )?;
        let result = exec_checked(
            self.session,
            &format!("prepare {}", self.spec.label),
            &tracefs_prepare_command(self.spec),
        );
        if let Err(error) = result {
            let cleanup = exec_checked(
                self.session,
                &format!("clean failed {} preparation", self.spec.label),
                &tracefs_cleanup_command(self.spec),
            );
            return match cleanup {
                Ok(()) => Err(error),
                Err(cleanup) => Err(format!(
                    "{} preparation failed: {error}; cleanup failed: {cleanup}",
                    self.spec.label
                )
                .into()),
            };
        }
        Ok(())
    }

    pub(super) fn start(&self) -> Result<()> {
        exec_checked(
            self.session,
            &format!("start {}", self.spec.label),
            &tracefs_control_command(self.spec, true),
        )
    }

    pub(super) fn stop(&self) -> Result<()> {
        exec_checked(
            self.session,
            &format!("stop {}", self.spec.label),
            &tracefs_control_command(self.spec, false),
        )
    }

    pub(super) fn retain(&self, filename: &str) -> Result<RetainedTrace> {
        exec_checked(
            self.session,
            &format!("retain {}", self.spec.label),
            &tracefs_retain_command(self.spec),
        )?;
        let raw_path = self.output_dir.join(filename);
        get(
            self.session,
            &format!("{}/trace.txt", self.spec.remote_root),
            &raw_path,
        )?;
        let stats = remote_read(
            self.session,
            &format!("{}/trace-stats.txt", self.spec.remote_root),
        )
        .ok_or_else(|| format!("{} did not retain trace statistics", self.spec.label))?;
        let capabilities = remote_read(
            self.session,
            &format!("{}/capabilities.tsv", self.spec.remote_root),
        )
        .ok_or_else(|| format!("{} did not retain event capabilities", self.spec.label))?;
        fs::write(self.output_dir.join("trace-stats.txt"), &stats)?;
        fs::write(
            self.output_dir.join("trace-capabilities.tsv"),
            &capabilities,
        )?;
        let sha256 = file_sha256(raw_path.clone())?;
        Ok(RetainedTrace {
            raw_path,
            stats,
            capabilities,
            sha256,
        })
    }

    pub(super) fn cleanup(&self) -> Result<()> {
        exec_checked(
            self.session,
            &format!("clean {}", self.spec.label),
            &tracefs_cleanup_command(self.spec),
        )
    }
}

fn event_enable_path(instance: &str, event: &str) -> String {
    let (system, name) = event.split_once(':').expect("static trace event");
    format!("{TRACEFS_MOUNT}/instances/{instance}/events/{system}/{name}/enable")
}

fn tracefs_prepare_command(spec: TracefsCaptureSpec) -> String {
    let root = sh(spec.remote_root);
    let mount_marker = sh(&format!("{}/owned-tracefs.mount", spec.remote_root));
    let instance_marker = sh(&format!("{}/owned-instance", spec.remote_root));
    let capabilities = sh(&format!("{}/capabilities.tsv", spec.remote_root));
    let instance = sh(&format!("{TRACEFS_MOUNT}/instances/{}", spec.instance));
    let mut checks = String::new();
    let mut enables = String::new();
    for event in spec.required_events {
        checks.push_str(&format!(
            "grep -qx {event} {mount}/available_events; printf '%s\\t%s\\n' {event} required >> {capabilities}; ",
            event = sh(event),
            mount = sh(TRACEFS_MOUNT),
            capabilities = capabilities,
        ));
        enables.push_str(&format!(
            "printf '1\\n' > {path}; ",
            path = sh(&event_enable_path(spec.instance, event)),
        ));
    }
    for event in spec.optional_events {
        checks.push_str(&format!(
            "if grep -qx {event} {mount}/available_events; then printf '%s\\t%s\\n' {event} enabled >> {capabilities}; printf '1\\n' > {path}; else printf '%s\\t%s\\n' {event} missing >> {capabilities}; fi; ",
            event = sh(event),
            mount = sh(TRACEFS_MOUNT),
            capabilities = capabilities,
            path = sh(&event_enable_path(spec.instance, event)),
        ));
    }
    format!(
        "set -eu; root={root}; mkdir -p \"$root\"; current=$(awk '$2 == \"{mount_path}\" && $3 == \"tracefs\" {{ print }}' /proc/mounts); if test -z \"$current\"; then mount -t tracefs tracefs {mount}; awk '$2 == \"{mount_path}\" && $3 == \"tracefs\" {{ print }}' /proc/mounts > {mount_marker}; test -s {mount_marker}; fi; test -d {mount}/instances; test ! -e {instance}; mkdir {instance}; printf '%s\\n' {instance} > {instance_marker}; : > {capabilities}; {checks} grep -qw mono {instance}/trace_clock; printf 'mono\\n' > {instance}/trace_clock; printf 'nop\\n' > {instance}/current_tracer; printf '{buffer_kb}\\n' > {instance}/buffer_size_kb; printf '0\\n' > {instance}/tracing_on; printf '0\\n' > {instance}/events/enable; : > {instance}/trace; {enables} test \"$(cat {instance}/tracing_on)\" = 0",
        root = root,
        mount_path = TRACEFS_MOUNT,
        mount = sh(TRACEFS_MOUNT),
        mount_marker = mount_marker,
        instance = instance,
        instance_marker = instance_marker,
        capabilities = capabilities,
        checks = checks,
        enables = enables,
        buffer_kb = TRACE_BUFFER_KB,
    )
}

fn tracefs_control_command(spec: TracefsCaptureSpec, start: bool) -> String {
    let instance = sh(&format!("{TRACEFS_MOUNT}/instances/{}", spec.instance));
    if start {
        format!(
            "set -eu; test -d {instance}; test \"$(cat {instance}/tracing_on)\" = 0; : > {instance}/trace; printf '1\\n' > {instance}/tracing_on; printf 'mister-magik-start\\n' > {instance}/trace_marker; test \"$(cat {instance}/tracing_on)\" = 1",
        )
    } else {
        format!(
            "set -eu; test -d {instance}; printf 'mister-magik-end\\n' > {instance}/trace_marker; printf '0\\n' > {instance}/tracing_on; test \"$(cat {instance}/tracing_on)\" = 0",
        )
    }
}

fn tracefs_retain_command(spec: TracefsCaptureSpec) -> String {
    let root = sh(spec.remote_root);
    let instance = sh(&format!("{TRACEFS_MOUNT}/instances/{}", spec.instance));
    format!(
        "set -eu; test \"$(cat {instance}/tracing_on)\" = 0; cat {instance}/trace > {root}/trace.txt; bytes=$(wc -c < {root}/trace.txt); test \"$bytes\" -gt 0; test \"$bytes\" -le {max_bytes}; : > {root}/trace-stats.txt; for stats in {instance}/per_cpu/cpu*/stats; do printf '== %s ==\\n' \"$stats\" >> {root}/trace-stats.txt; cat \"$stats\" >> {root}/trace-stats.txt; done; test -s {root}/trace-stats.txt",
        max_bytes = TRACE_MAX_BYTES,
    )
}

fn tracefs_cleanup_command(spec: TracefsCaptureSpec) -> String {
    let root = sh(spec.remote_root);
    let mount_marker = sh(&format!("{}/owned-tracefs.mount", spec.remote_root));
    let instance_marker = sh(&format!("{}/owned-instance", spec.remote_root));
    let instance_path = format!("{TRACEFS_MOUNT}/instances/{}", spec.instance);
    let instance = sh(&instance_path);
    format!(
        "set -eu; root={root}; current=$(awk '$2 == \"{mount_path}\" && $3 == \"tracefs\" {{ print }}' /proc/mounts); if test -n \"$current\" && test -e {instance}; then test -f {instance_marker}; test \"$(cat {instance_marker})\" = {instance}; printf '0\\n' > {instance}/tracing_on; printf '0\\n' > {instance}/events/enable; : > {instance}/trace; i=0; while ! rmdir {instance} 2>/dev/null && test \"$i\" -lt 50; do i=$((i+1)); sleep 0.1; done; test ! -e {instance}; fi; if test -f {mount_marker}; then current=$(awk '$2 == \"{mount_path}\" && $3 == \"tracefs\" {{ print }}' /proc/mounts); owned=$(cat {mount_marker}); if test -n \"$current\"; then test \"$current\" = \"$owned\"; i=0; while ! umount {mount} 2>/dev/null && test \"$i\" -lt 50; do i=$((i+1)); sleep 0.1; done; current=$(awk '$2 == \"{mount_path}\" && $3 == \"tracefs\" {{ print }}' /proc/mounts); test -z \"$current\"; fi; fi; rm -rf \"$root\"; test ! -e \"$root\"",
        root = root,
        mount_path = TRACEFS_MOUNT,
        instance = instance,
        instance_marker = instance_marker,
        mount_marker = mount_marker,
        mount = sh(TRACEFS_MOUNT),
    )
}

#[derive(Clone, Debug)]
struct ParsedEvent {
    timestamp_ns: u64,
    cpu: usize,
    name: String,
    payload: String,
}

#[derive(Default)]
struct ThreadMetrics {
    name: String,
    on_cpu_ns: u64,
    runnable_ns: Vec<u64>,
    preemptions: u64,
    migrations: u64,
    switches: u64,
}

#[derive(Default)]
struct SpanMetrics {
    count: u64,
    total_ns: u64,
    max_ns: u64,
}

pub(super) fn summarize_scheduler_trace(
    trace: &str,
    stats: &str,
) -> Result<(Value, String, String, String)> {
    let mut events = trace
        .lines()
        .filter_map(parse_event)
        .collect::<Vec<ParsedEvent>>();
    if events.is_empty() {
        return Err("scheduler trace contains no parseable events".into());
    }
    events.sort_by_key(|event| (event.timestamp_ns, event.cpu));
    let start_ns = events
        .iter()
        .find(|event| {
            event.name == "tracing_mark_write" && event.payload.contains("mister-magik-start")
        })
        .map(|event| event.timestamp_ns)
        .unwrap_or(events[0].timestamp_ns);
    let end_ns = events
        .iter()
        .rev()
        .find(|event| {
            event.name == "tracing_mark_write" && event.payload.contains("mister-magik-end")
        })
        .map(|event| event.timestamp_ns)
        .unwrap_or_else(|| events.last().expect("non-empty").timestamp_ns);
    if end_ns <= start_ns {
        return Err("scheduler trace timestamps are not increasing".into());
    }
    let overruns = trace_overruns(stats)?;
    if overruns != 0 {
        return Err(format!("scheduler trace reported {overruns} buffer overruns").into());
    }

    let cpu_count = events.iter().map(|event| event.cpu).max().unwrap_or(0) + 1;
    let mut running = vec![None::<(u32, u64)>; cpu_count];
    let mut busy_intervals = vec![Vec::<(u64, u64)>::new(); cpu_count];
    let mut wake_at = HashMap::<u32, u64>::new();
    let mut threads = BTreeMap::<u32, ThreadMetrics>::new();
    let mut irq_open = vec![Vec::<(String, u64)>::new(); cpu_count];
    let mut softirq_open = vec![Vec::<(String, u64)>::new(); cpu_count];
    let mut irq = BTreeMap::<String, SpanMetrics>::new();
    let mut softirq = BTreeMap::<String, SpanMetrics>::new();

    for event in events
        .iter()
        .filter(|event| event.timestamp_ns >= start_ns && event.timestamp_ns <= end_ns)
    {
        match event.name.as_str() {
            "sched_waking" | "sched_wakeup" => {
                if let Some(pid) = field_u32(&event.payload, "pid") {
                    wake_at.entry(pid).or_insert(event.timestamp_ns);
                    update_name(&mut threads, pid, field_text(&event.payload, "comm"));
                }
            }
            "sched_migrate_task" => {
                if let Some(pid) = field_u32(&event.payload, "pid") {
                    threads.entry(pid).or_default().migrations += 1;
                    update_name(&mut threads, pid, field_text(&event.payload, "comm"));
                }
            }
            "sched_switch" => {
                let prev_pid = field_u32(&event.payload, "prev_pid").unwrap_or(0);
                let next_pid = field_u32(&event.payload, "next_pid").unwrap_or(0);
                update_name(
                    &mut threads,
                    prev_pid,
                    text_between(&event.payload, "prev_comm=", " prev_pid="),
                );
                update_name(
                    &mut threads,
                    next_pid,
                    text_between(&event.payload, "next_comm=", " next_pid="),
                );
                if let Some((running_pid, began)) = running[event.cpu].take() {
                    let elapsed = event.timestamp_ns.saturating_sub(began);
                    if running_pid != 0 {
                        threads.entry(running_pid).or_default().on_cpu_ns += elapsed;
                        busy_intervals[event.cpu].push((began, event.timestamp_ns));
                    }
                }
                if prev_pid != 0 {
                    let thread = threads.entry(prev_pid).or_default();
                    thread.switches += 1;
                    if field_text(&event.payload, "prev_state")
                        .is_some_and(|state| state.starts_with('R') || state.starts_with('r'))
                    {
                        thread.preemptions += 1;
                        wake_at.insert(prev_pid, event.timestamp_ns);
                    }
                }
                if next_pid != 0 {
                    let thread = threads.entry(next_pid).or_default();
                    thread.switches += 1;
                    if let Some(woke) = wake_at.remove(&next_pid) {
                        thread
                            .runnable_ns
                            .push(event.timestamp_ns.saturating_sub(woke));
                    }
                }
                running[event.cpu] = Some((next_pid, event.timestamp_ns));
            }
            "irq_handler_entry" => {
                let name = field_text(&event.payload, "name")
                    .unwrap_or("unknown")
                    .to_owned();
                irq_open[event.cpu].push((name, event.timestamp_ns));
            }
            "irq_handler_exit" => {
                close_span(&mut irq_open[event.cpu], &mut irq, event.timestamp_ns)
            }
            "softirq_entry" => {
                let vector = field_text(&event.payload, "vec")
                    .unwrap_or("unknown")
                    .to_owned();
                softirq_open[event.cpu].push((vector, event.timestamp_ns));
            }
            "softirq_exit" => close_span(
                &mut softirq_open[event.cpu],
                &mut softirq,
                event.timestamp_ns,
            ),
            _ => {}
        }
    }
    for (cpu, current) in running.into_iter().enumerate() {
        if let Some((pid, began)) = current {
            if pid != 0 && end_ns > began {
                threads.entry(pid).or_default().on_cpu_ns += end_ns - began;
                busy_intervals[cpu].push((began, end_ns));
            }
        }
    }

    let duration_ns = end_ns - start_ns;
    let mut thread_rows = threads
        .into_iter()
        .filter(|(pid, _)| *pid != 0)
        .collect::<Vec<_>>();
    thread_rows.sort_by_key(|(_, metrics)| std::cmp::Reverse(metrics.on_cpu_ns));
    let thread_json = thread_rows
        .iter_mut()
        .map(|(pid, metrics)| {
            metrics.runnable_ns.sort_unstable();
            json!({
                "pid": pid,
                "name": metrics.name,
                "on_cpu_us": metrics.on_cpu_ns / 1_000,
                "runnable_delay_us": percentile_values(&metrics.runnable_ns),
                "preemptions": metrics.preemptions,
                "migrations": metrics.migrations,
                "switches": metrics.switches,
            })
        })
        .collect::<Vec<_>>();
    let cpu_json = busy_intervals
        .iter()
        .enumerate()
        .map(|(cpu, intervals)| {
            let busy_ns = intervals
                .iter()
                .map(|(start, end)| end.saturating_sub(*start))
                .sum::<u64>();
            json!({
                "cpu": cpu,
                "busy_us": busy_ns / 1_000,
                "busy_pct": busy_ns as f64 * 100.0 / duration_ns as f64,
            })
        })
        .collect::<Vec<_>>();
    let overlap_ns = if busy_intervals.len() >= 2 {
        interval_overlap(&busy_intervals[0], &busy_intervals[1])
    } else {
        0
    };
    let summary = json!({
        "duration_us": duration_ns / 1_000,
        "event_count": events.len(),
        "trace_overruns": overruns,
        "cpu_count": cpu_count,
        "dual_core_overlap_us": overlap_ns / 1_000,
        "dual_core_overlap_pct": overlap_ns as f64 * 100.0 / duration_ns as f64,
        "cpus": cpu_json,
        "threads": thread_json,
        "irq": span_json(&irq),
        "softirq": span_json(&softirq),
    });
    Ok((
        summary,
        thread_tsv(&thread_rows),
        cpu_tsv(&busy_intervals, duration_ns),
        span_tsv(&irq, &softirq),
    ))
}

fn parse_event(line: &str) -> Option<ParsedEvent> {
    let open = line.find('[')?;
    let close = line[open + 1..].find(']')? + open + 1;
    let cpu = line[open + 1..close].trim().parse::<usize>().ok()?;
    let remainder = &line[close + 1..];
    let separator = remainder.find(": ")?;
    let prefix = &remainder[..separator];
    let timestamp = prefix.split_whitespace().last()?;
    let timestamp_ns = parse_timestamp_ns(timestamp)?;
    let event = &remainder[separator + 2..];
    let (name, payload) = event.split_once(": ").unwrap_or((event, ""));
    Some(ParsedEvent {
        timestamp_ns,
        cpu,
        name: name.trim().to_owned(),
        payload: payload.trim().to_owned(),
    })
}

fn parse_timestamp_ns(value: &str) -> Option<u64> {
    let (seconds, fraction) = value.split_once('.')?;
    let seconds = seconds.parse::<u64>().ok()?;
    let mut fraction = fraction.bytes().take(9).collect::<Vec<_>>();
    if !fraction.iter().all(u8::is_ascii_digit) {
        return None;
    }
    while fraction.len() < 9 {
        fraction.push(b'0');
    }
    let nanos = std::str::from_utf8(&fraction).ok()?.parse::<u64>().ok()?;
    seconds.checked_mul(1_000_000_000)?.checked_add(nanos)
}

fn field_text<'a>(payload: &'a str, key: &str) -> Option<&'a str> {
    let start = payload.find(&format!("{key}="))? + key.len() + 1;
    Some(payload[start..].split_whitespace().next()?)
}

fn field_u32(payload: &str, key: &str) -> Option<u32> {
    field_text(payload, key)?.parse().ok()
}

fn text_between<'a>(payload: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let start = payload.find(prefix)? + prefix.len();
    let end = payload[start..].find(suffix)? + start;
    Some(payload[start..end].trim())
}

fn update_name(threads: &mut BTreeMap<u32, ThreadMetrics>, pid: u32, name: Option<&str>) {
    if pid == 0 {
        return;
    }
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        threads.entry(pid).or_default().name = name.to_owned();
    }
}

fn close_span(
    open: &mut Vec<(String, u64)>,
    metrics: &mut BTreeMap<String, SpanMetrics>,
    end_ns: u64,
) {
    if let Some((name, start_ns)) = open.pop() {
        let elapsed = end_ns.saturating_sub(start_ns);
        let metric = metrics.entry(name).or_default();
        metric.count += 1;
        metric.total_ns += elapsed;
        metric.max_ns = metric.max_ns.max(elapsed);
    }
}

fn trace_overruns(stats: &str) -> Result<u64> {
    let mut total = 0u64;
    for line in stats.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("overrun:") {
            total = total.saturating_add(value.trim().parse::<u64>()?);
        }
        if let Some(value) = line.strip_prefix("commit overrun:") {
            total = total.saturating_add(value.trim().parse::<u64>()?);
        }
    }
    Ok(total)
}

fn percentile_values(values: &[u64]) -> Value {
    json!({
        "samples": values.len(),
        "median": percentile(values, 50) / 1_000,
        "p95": percentile(values, 95) / 1_000,
        "p99": percentile(values, 99) / 1_000,
        "max": values.last().copied().unwrap_or(0) / 1_000,
    })
}

fn percentile(values: &[u64], percent: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let index = (values.len() - 1) * percent / 100;
    values[index]
}

fn interval_overlap(left: &[(u64, u64)], right: &[(u64, u64)]) -> u64 {
    let (mut i, mut j, mut total) = (0, 0, 0u64);
    while i < left.len() && j < right.len() {
        let start = left[i].0.max(right[j].0);
        let end = left[i].1.min(right[j].1);
        total = total.saturating_add(end.saturating_sub(start));
        if left[i].1 <= right[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    total
}

fn span_json(spans: &BTreeMap<String, SpanMetrics>) -> Value {
    Value::Array(
        spans
            .iter()
            .map(|(name, metric)| {
                json!({
                    "name": name,
                    "count": metric.count,
                    "total_us": metric.total_ns / 1_000,
                    "max_us": metric.max_ns / 1_000,
                })
            })
            .collect(),
    )
}

fn thread_tsv(rows: &[(u32, ThreadMetrics)]) -> String {
    let mut output = String::from(
        "pid\tname\ton_cpu_us\trunnable_samples\trunnable_p95_us\tpreemptions\tmigrations\tswitches\n",
    );
    for (pid, metrics) in rows {
        output.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            pid,
            metrics.name.replace(['\t', '\n'], " "),
            metrics.on_cpu_ns / 1_000,
            metrics.runnable_ns.len(),
            percentile(&metrics.runnable_ns, 95) / 1_000,
            metrics.preemptions,
            metrics.migrations,
            metrics.switches,
        ));
    }
    output
}

fn cpu_tsv(intervals: &[Vec<(u64, u64)>], duration_ns: u64) -> String {
    let mut output = String::from("cpu\tbusy_us\tbusy_pct\n");
    for (cpu, intervals) in intervals.iter().enumerate() {
        let busy_ns = intervals
            .iter()
            .map(|(start, end)| end.saturating_sub(*start))
            .sum::<u64>();
        output.push_str(&format!(
            "{}\t{}\t{:.3}\n",
            cpu,
            busy_ns / 1_000,
            busy_ns as f64 * 100.0 / duration_ns as f64,
        ));
    }
    output
}

fn span_tsv(
    irq: &BTreeMap<String, SpanMetrics>,
    softirq: &BTreeMap<String, SpanMetrics>,
) -> String {
    let mut output = String::from("kind\tname\tcount\ttotal_us\tmax_us\n");
    for (kind, spans) in [("irq", irq), ("softirq", softirq)] {
        for (name, metric) in spans {
            output.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                kind,
                name.replace(['\t', '\n'], " "),
                metric.count,
                metric.total_ns / 1_000,
                metric.max_ns / 1_000,
            ));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_parser_attributes_two_cpus_wakeups_and_irqs() {
        let trace = r#"
 marker-1 [000] .... 10.000000000: tracing_mark_write: mister-magik-start
 idle-0 [000] .... 10.001000000: sched_wakeup: comm=ui pid=10 prio=120 target_cpu=000
 idle-0 [000] .... 10.002000000: sched_switch: prev_comm=swapper/0 prev_pid=0 prev_prio=120 prev_state=R ==> next_comm=ui next_pid=10 next_prio=120
 idle-0 [001] .... 10.002500000: sched_switch: prev_comm=swapper/1 prev_pid=0 prev_prio=120 prev_state=R ==> next_comm=worker next_pid=20 next_prio=120
 ui-10 [000] .... 10.003000000: irq_handler_entry: irq=42 name=mmc0
 ui-10 [000] .... 10.003100000: irq_handler_exit: irq=42 ret=handled
 ui-10 [000] .... 10.005000000: sched_migrate_task: comm=ui pid=10 prio=120 orig_cpu=0 dest_cpu=1
 ui-10 [000] .... 10.006000000: sched_switch: prev_comm=ui prev_pid=10 prev_prio=120 prev_state=R+ ==> next_comm=swapper/0 next_pid=0 next_prio=120
 worker-20 [001] .... 10.008000000: sched_switch: prev_comm=worker prev_pid=20 prev_prio=120 prev_state=S ==> next_comm=ui next_pid=10 next_prio=120
 ui-10 [001] .... 10.010000000: sched_switch: prev_comm=ui prev_pid=10 prev_prio=120 prev_state=S ==> next_comm=swapper/1 next_pid=0 next_prio=120
 marker-1 [000] .... 10.011000000: tracing_mark_write: mister-magik-end
"#;
        let stats = "== cpu0 ==\noverrun: 0\ncommit overrun: 0\n== cpu1 ==\noverrun: 0\ncommit overrun: 0\n";
        let (summary, threads, cpus, irqs) = summarize_scheduler_trace(trace, stats).unwrap();
        assert_eq!(summary["cpu_count"], 2);
        assert!(summary["dual_core_overlap_us"].as_u64().unwrap() > 0);
        assert!(threads.contains("ui"));
        assert!(cpus.contains("cpu\tbusy_us"));
        assert!(irqs.contains("mmc0"));
        let ui = summary["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|thread| thread["pid"] == 10)
            .unwrap();
        assert_eq!(ui["migrations"], 1);
        assert_eq!(ui["preemptions"], 1);
        assert_eq!(ui["runnable_delay_us"]["max"], 2_000);
    }

    #[test]
    fn scheduler_parser_rejects_overruns_and_empty_input() {
        assert!(summarize_scheduler_trace("", "overrun: 0").is_err());
        let trace = "a-1 [000] .... 1.000000000: sched_switch: prev_comm=a prev_pid=1 prev_prio=1 prev_state=R ==> next_comm=b next_pid=2 next_prio=1\na-1 [000] .... 1.100000000: sched_switch: prev_comm=b prev_pid=2 prev_prio=1 prev_state=S ==> next_comm=a next_pid=1 next_prio=1\n";
        assert!(summarize_scheduler_trace(trace, "overrun: 1").is_err());
    }

    #[test]
    fn generated_commands_are_instance_owned_and_bounded() {
        let prepare = tracefs_prepare_command(SCHEDULER_TRACE_SPEC);
        let cleanup = tracefs_cleanup_command(SCHEDULER_TRACE_SPEC);
        let retain = tracefs_retain_command(SCHEDULER_TRACE_SPEC);
        assert!(prepare.contains("instances/mister-magik-scheduler"));
        assert!(prepare.contains("owned-instance"));
        assert!(cleanup.contains("owned-tracefs.mount"));
        assert!(retain.contains(&TRACE_MAX_BYTES.to_string()));
        assert!(!prepare.contains("/media/fat"));
    }
}
