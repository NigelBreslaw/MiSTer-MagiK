// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const TRACEFS_MOUNT: &str = "/sys/kernel/tracing";
const TRACE_MAX_BYTES: u64 = 64 * 1024 * 1024;
const FUNCTION_GRAPH_BUFFER_KB: u64 = 16_384;
const FUNCTION_GRAPH_MAX_DEPTH: u8 = 4;

pub(super) const SCHEDULER_TRACE_SPEC: TracefsCaptureSpec = TracefsCaptureSpec {
    label: "scheduler trace",
    instance: "mister-magik-scheduler",
    remote_root: "/tmp/mister-magik/scheduler-trace",
    buffer_kb: 16_384,
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
    mode: TracefsCaptureMode::Events,
};

pub(super) const STORAGE_TRACE_SPEC: TracefsCaptureSpec = TracefsCaptureSpec {
    label: "storage attribution trace",
    instance: "mister-magik-storage",
    remote_root: "/tmp/mister-magik/storage-attribution",
    buffer_kb: 16_384,
    required_events: &[
        "block:block_rq_issue",
        "block:block_rq_complete",
        "sched:sched_process_fork",
        "sched:sched_process_exit",
    ],
    optional_events: &[
        "syscalls:sys_enter_openat",
        "syscalls:sys_exit_openat",
        "syscalls:sys_enter_getdents64",
        "syscalls:sys_exit_getdents64",
        "syscalls:sys_enter_fsync",
        "syscalls:sys_exit_fsync",
        "syscalls:sys_enter_fdatasync",
        "syscalls:sys_exit_fdatasync",
        "syscalls:sys_enter_renameat",
        "syscalls:sys_exit_renameat",
        "syscalls:sys_enter_renameat2",
        "syscalls:sys_exit_renameat2",
        "syscalls:sys_enter_mkdirat",
        "syscalls:sys_exit_mkdirat",
        "syscalls:sys_enter_unlinkat",
        "syscalls:sys_exit_unlinkat",
    ],
    mode: TracefsCaptureMode::Events,
};

#[derive(Clone, Copy)]
pub(super) struct TracefsFunctionGroup {
    pub(super) label: &'static str,
    pub(super) functions: &'static [&'static str],
}

#[derive(Clone, Copy)]
pub(super) enum TracefsCaptureMode {
    Events,
    FunctionGraph {
        function_groups: &'static [TracefsFunctionGroup],
    },
}

#[derive(Clone, Copy)]
pub(super) struct TracefsCaptureSpec {
    pub(super) label: &'static str,
    pub(super) instance: &'static str,
    pub(super) remote_root: &'static str,
    pub(super) buffer_kb: u64,
    pub(super) required_events: &'static [&'static str],
    pub(super) optional_events: &'static [&'static str],
    pub(super) mode: TracefsCaptureMode,
}

impl TracefsCaptureSpec {
    pub(super) const fn function_graph(
        label: &'static str,
        instance: &'static str,
        remote_root: &'static str,
        function_groups: &'static [TracefsFunctionGroup],
    ) -> Self {
        Self {
            label,
            instance,
            remote_root,
            buffer_kb: FUNCTION_GRAPH_BUFFER_KB,
            required_events: &[],
            optional_events: &[],
            mode: TracefsCaptureMode::FunctionGraph { function_groups },
        }
    }
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
        validate_tracefs_spec(self.spec)?;
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
        let overruns = trace_overruns(&stats)?;
        if overruns != 0 {
            return Err(format!("{} trace has {overruns} overrun records", self.spec.label).into());
        }
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

fn validate_tracefs_spec(spec: TracefsCaptureSpec) -> Result<()> {
    if spec.instance.is_empty()
        || !spec
            .instance
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("invalid tracefs instance name {}", spec.instance).into());
    }
    if spec.buffer_kb == 0 {
        return Err("tracefs buffer size must be non-zero".into());
    }
    if let TracefsCaptureMode::FunctionGraph { function_groups } = spec.mode {
        if spec.buffer_kb != FUNCTION_GRAPH_BUFFER_KB {
            return Err(format!(
                "function-graph buffer must be {FUNCTION_GRAPH_BUFFER_KB} KiB per CPU"
            )
            .into());
        }
        if !spec.required_events.is_empty() || !spec.optional_events.is_empty() {
            return Err("function-graph captures cannot also enable trace events".into());
        }
        if function_groups.is_empty() {
            return Err("function-graph capture requires an allowlisted function group".into());
        }
        for group in function_groups {
            if group.label.is_empty()
                || !group
                    .label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                || group.functions.is_empty()
            {
                return Err("function-graph capture has an invalid function group".into());
            }
            for function in group.functions {
                if function.is_empty()
                    || !function
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
                {
                    return Err(format!(
                        "function-graph group {} contains an invalid function name",
                        group.label
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
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
    let tracer_setup = match spec.mode {
        TracefsCaptureMode::Events => {
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
            format!("{enables} test \"$(cat {instance}/current_tracer)\" = nop")
        }
        TracefsCaptureMode::FunctionGraph { function_groups } => {
            let resolved = sh(&format!("{}/resolved-functions.txt", spec.remote_root));
            checks.push_str(&format!(
                "grep -qw function_graph {mount}/available_tracers; test -r {mount}/available_filter_functions; test -w {instance}/set_graph_function; test -w {instance}/max_graph_depth; printf '%s\\t%s\\n' tracer:function_graph required >> {capabilities}; : > {resolved}; ",
                mount = sh(TRACEFS_MOUNT),
            ));
            for group in function_groups {
                let candidates = group
                    .functions
                    .iter()
                    .map(|function| sh(function))
                    .collect::<Vec<_>>()
                    .join(" ");
                checks.push_str(&format!(
                    "group_found=0; for function in {candidates}; do if awk -v wanted=\"$function\" '$1 == wanted {{ found=1 }} END {{ exit !found }}' {mount}/available_filter_functions; then printf '%s\\n' \"$function\" >> {resolved}; printf 'function:%s:%s\\tresolved\\n' {group_label} \"$function\" >> {capabilities}; group_found=1; fi; done; test \"$group_found\" = 1; printf '%s\\t%s\\n' {group_capability} resolved >> {capabilities}; ",
                    mount = sh(TRACEFS_MOUNT),
                    group_label = sh(group.label),
                    group_capability = sh(&format!("function-group:{}", group.label)),
                ));
            }
            format!(
                "sort -u {resolved} > {resolved}.sorted; mv {resolved}.sorted {resolved}; test -s {resolved}; cat {resolved} > {instance}/set_graph_function; printf '{depth}\\n' > {instance}/max_graph_depth; printf 'function_graph\\n' > {instance}/current_tracer; test \"$(cat {instance}/current_tracer)\" = function_graph; test \"$(cat {instance}/max_graph_depth)\" = {depth}",
                depth = FUNCTION_GRAPH_MAX_DEPTH,
            )
        }
    };
    format!(
        "set -eu; root={root}; mkdir -p \"$root\"; current=$(awk '$2 == \"{mount_path}\" && $3 == \"tracefs\" {{ print }}' /proc/mounts); if test -z \"$current\"; then mount -t tracefs tracefs {mount}; awk '$2 == \"{mount_path}\" && $3 == \"tracefs\" {{ print }}' /proc/mounts > {mount_marker}; test -s {mount_marker}; fi; test -d {mount}/instances; test ! -e {instance}; mkdir {instance}; printf '%s\\n' {instance} > {instance_marker}; : > {capabilities}; grep -qw mono {instance}/trace_clock; printf 'mono\\n' > {instance}/trace_clock; printf 'nop\\n' > {instance}/current_tracer; printf '{buffer_kb}\\n' > {instance}/buffer_size_kb; printf '0\\n' > {instance}/tracing_on; printf '0\\n' > {instance}/events/enable; : > {instance}/trace; {checks} {tracer_setup}; test \"$(cat {instance}/tracing_on)\" = 0",
        root = root,
        mount_path = TRACEFS_MOUNT,
        mount = sh(TRACEFS_MOUNT),
        mount_marker = mount_marker,
        instance = instance,
        instance_marker = instance_marker,
        capabilities = capabilities,
        checks = checks,
        tracer_setup = tracer_setup,
        buffer_kb = spec.buffer_kb,
    )
}

fn tracefs_control_command(spec: TracefsCaptureSpec, start: bool) -> String {
    let instance = sh(&format!("{TRACEFS_MOUNT}/instances/{}", spec.instance));
    let tracer = match spec.mode {
        TracefsCaptureMode::Events => "nop",
        TracefsCaptureMode::FunctionGraph { .. } => "function_graph",
    };
    if start {
        format!(
            "set -eu; test -d {instance}; test \"$(cat {instance}/current_tracer)\" = {tracer}; test \"$(cat {instance}/tracing_on)\" = 0; : > {instance}/trace; printf '1\\n' > {instance}/tracing_on; printf 'mister-magik-start\\n' > {instance}/trace_marker; test \"$(cat {instance}/tracing_on)\" = 1",
            tracer = sh(tracer),
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
        "set -eu; root={root}; current=$(awk '$2 == \"{mount_path}\" && $3 == \"tracefs\" {{ print }}' /proc/mounts); if test -n \"$current\" && test -e {instance}; then test -f {instance_marker}; test \"$(cat {instance_marker})\" = {instance}; printf '0\\n' > {instance}/tracing_on; printf '0\\n' > {instance}/events/enable; printf 'nop\\n' > {instance}/current_tracer; if test -e {instance}/set_graph_function; then : > {instance}/set_graph_function; fi; if test -e {instance}/max_graph_depth; then printf '0\\n' > {instance}/max_graph_depth; fi; : > {instance}/trace; i=0; while ! rmdir {instance} 2>/dev/null && test \"$i\" -lt 50; do i=$((i+1)); sleep 0.1; done; test ! -e {instance}; fi; if test -f {mount_marker}; then current=$(awk '$2 == \"{mount_path}\" && $3 == \"tracefs\" {{ print }}' /proc/mounts); owned=$(cat {mount_marker}); if test -n \"$current\"; then test \"$current\" = \"$owned\"; i=0; while ! umount {mount} 2>/dev/null && test \"$i\" -lt 50; do i=$((i+1)); sleep 0.1; done; current=$(awk '$2 == \"{mount_path}\" && $3 == \"tracefs\" {{ print }}' /proc/mounts); test -z \"$current\"; fi; fi; rm -rf \"$root\"; test ! -e \"$root\"",
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
    context_pid: u32,
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
        if let Some((pid, began)) = current
            && pid != 0
            && end_ns > began
        {
            threads.entry(pid).or_default().on_cpu_ns += end_ns - began;
            busy_intervals[cpu].push((began, end_ns));
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

pub(super) fn summarize_storage_trace(trace: &str, stats: &str, root_pid: u32) -> Result<Value> {
    if root_pid == 0 {
        return Err("storage trace root PID is zero".into());
    }
    let mut events = trace.lines().filter_map(parse_event).collect::<Vec<_>>();
    if events.is_empty() {
        return Err("storage trace contains no parseable events".into());
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
        return Err("storage trace timestamps are not increasing".into());
    }
    let overruns = trace_overruns(stats)?;
    if overruns != 0 {
        return Err(format!("storage trace reported {overruns} buffer overruns").into());
    }
    let mut descendants = BTreeSet::from([root_pid]);
    let mut open = HashMap::<(u32, String), u64>::new();
    let mut syscalls = BTreeMap::<String, Vec<u64>>::new();
    let mut block_issue = 0u64;
    let mut block_complete = 0u64;
    for event in events
        .iter()
        .filter(|event| event.timestamp_ns >= start_ns && event.timestamp_ns <= end_ns)
    {
        if event.name == "sched_process_fork" {
            let parent = field_u32(&event.payload, "parent_pid")
                .or_else(|| field_u32(&event.payload, "pid"))
                .unwrap_or(0);
            let child = field_u32(&event.payload, "child_pid").unwrap_or(0);
            if descendants.contains(&parent) && child != 0 {
                descendants.insert(child);
            }
            continue;
        }
        match event.name.as_str() {
            "block_rq_issue" => block_issue += 1,
            "block_rq_complete" => block_complete += 1,
            name if name.starts_with("sys_enter_") && descendants.contains(&event.context_pid) => {
                let syscall = name.trim_start_matches("sys_enter_").to_owned();
                open.insert((event.context_pid, syscall), event.timestamp_ns);
            }
            name if name.starts_with("sys_exit_") && descendants.contains(&event.context_pid) => {
                let syscall = name.trim_start_matches("sys_exit_").to_owned();
                if let Some(started) = open.remove(&(event.context_pid, syscall.clone())) {
                    syscalls
                        .entry(syscall)
                        .or_default()
                        .push(event.timestamp_ns.saturating_sub(started));
                }
            }
            _ => {}
        }
    }
    let mut syscall_rows = syscalls.into_iter().collect::<Vec<_>>();
    for (_, durations) in &mut syscall_rows {
        durations.sort_unstable();
    }
    syscall_rows
        .sort_by_key(|(_, durations)| std::cmp::Reverse(durations.iter().copied().sum::<u64>()));
    Ok(json!({
        "duration_us": (end_ns - start_ns) / 1_000,
        "event_count": events.len(),
        "trace_overruns": overruns,
        "root_pid": root_pid,
        "descendant_tids": descendants,
        "block": {
            "request_issues": block_issue,
            "request_completions": block_complete,
        },
        "syscalls": syscall_rows.into_iter().map(|(name, durations)| {
            let total_ns = durations.iter().sum::<u64>();
            json!({
                "name": name,
                "count": durations.len(),
                "total_us": total_ns / 1_000,
                "median_us": percentile(&durations, 50) / 1_000,
                "p95_us": percentile(&durations, 95) / 1_000,
                "p99_us": percentile(&durations, 99) / 1_000,
                "max_us": durations.last().copied().unwrap_or(0) / 1_000,
            })
        }).collect::<Vec<_>>(),
    }))
}

pub(super) fn summarize_function_graph_trace(trace: &str, stats: &str) -> Result<Value> {
    let overruns = trace_overruns(stats)?;
    if overruns != 0 {
        return Err(format!("function graph reported {overruns} buffer overruns").into());
    }
    let mut calls = BTreeMap::<String, (u64, u64, u64)>::new();
    let mut traced_lines = 0u64;
    for line in trace.lines() {
        let Some((left, right)) = line.split_once('|') else {
            continue;
        };
        let Some(function) = function_graph_function(right) else {
            continue;
        };
        traced_lines += 1;
        let duration_ns = function_graph_duration_ns(left).unwrap_or(0);
        let entry = calls.entry(function).or_default();
        entry.0 += 1;
        entry.1 = entry.1.saturating_add(duration_ns);
        entry.2 = entry.2.max(duration_ns);
    }
    if calls.is_empty() {
        return Err("function graph contains no parseable function records".into());
    }
    let mut functions = calls
        .into_iter()
        .map(|(function, (records, total_ns, max_ns))| {
            json!({
                "function": function,
                "records": records,
                "timed_total_us": total_ns / 1_000,
                "timed_max_us": max_ns / 1_000,
            })
        })
        .collect::<Vec<_>>();
    functions.sort_by_key(|row| {
        std::cmp::Reverse(
            row.get("timed_total_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        )
    });
    functions.truncate(25);
    Ok(json!({
        "trace_overruns": overruns,
        "parsed_records": traced_lines,
        "top_functions": functions,
        "duration_semantics": "function_graph inclusive durations when emitted; entry-only records have zero duration",
    }))
}

fn function_graph_function(right: &str) -> Option<String> {
    let right = right.trim();
    if let Some(comment) = right.strip_prefix("} /* ") {
        return comment.strip_suffix(" */").map(str::to_owned);
    }
    let open = right.find('(')?;
    let name = right[..open].trim();
    (!name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.')))
    .then(|| name.to_owned())
}

fn function_graph_duration_ns(left: &str) -> Option<u64> {
    let fields = left.split_whitespace().collect::<Vec<_>>();
    let unit = *fields.last()?;
    let value = fields
        .get(fields.len().checked_sub(2)?)?
        .parse::<f64>()
        .ok()?;
    let multiplier = match unit {
        "ns" => 1.0,
        "us" => 1_000.0,
        "ms" => 1_000_000.0,
        "s" => 1_000_000_000.0,
        _ => return None,
    };
    Some((value * multiplier) as u64)
}

fn parse_event(line: &str) -> Option<ParsedEvent> {
    let open = line.find('[')?;
    let context = line[..open].split_whitespace().last()?;
    let context_pid = context.rsplit_once('-')?.1.parse::<u32>().ok()?;
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
        context_pid,
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
    payload[start..].split_whitespace().next()
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
    let mut seen = false;
    for line in stats.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("overrun:") {
            seen = true;
            total = total.saturating_add(value.trim().parse::<u64>()?);
        }
        if let Some(value) = line.strip_prefix("commit overrun:") {
            seen = true;
            total = total.saturating_add(value.trim().parse::<u64>()?);
        }
    }
    if !seen {
        return Err("trace statistics contain no overrun counters".into());
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
    fn storage_parser_tracks_descendant_syscalls_and_block_requests() {
        let trace = r#"
 marker-1 [000] .... 20.000000000: tracing_mark_write: mister-magik-start
 app-40 [000] .... 20.001000000: sched_process_fork: comm=app pid=40 child_comm=worker child_pid=41
 worker-41 [001] .... 20.002000000: sys_enter_fsync: fd=7
 mmcqd-8 [000] .... 20.002500000: block_rq_issue: 179,0 W 0 () 8 + 8 [worker]
 mmcqd-8 [000] .... 20.003000000: block_rq_complete: 179,0 W () 8 + 8 [0]
 worker-41 [001] .... 20.004000000: sys_exit_fsync: 0x0
 other-99 [000] .... 20.005000000: sys_enter_read: fd=3
 other-99 [000] .... 20.006000000: sys_exit_read: 4
 marker-1 [000] .... 20.007000000: tracing_mark_write: mister-magik-end
"#;
        let summary =
            summarize_storage_trace(trace, "overrun: 0\ncommit overrun: 0\n", 40).unwrap();
        assert_eq!(summary["block"]["request_issues"], 1);
        assert_eq!(summary["block"]["request_completions"], 1);
        assert_eq!(summary["syscalls"].as_array().map(Vec::len), Some(1));
        assert_eq!(summary["syscalls"][0]["name"], "fsync");
        assert_eq!(summary["syscalls"][0]["max_us"], 2_000);
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

    #[test]
    fn function_graph_commands_resolve_allowlisted_groups_and_restore_nop() {
        const GROUPS: &[TracefsFunctionGroup] = &[
            TracefsFunctionGroup {
                label: "directory-walk",
                functions: &["iterate_dir", "vfs_readdir"],
            },
            TracefsFunctionGroup {
                label: "durability",
                functions: &["vfs_fsync", "generic_file_fsync"],
            },
        ];
        let spec = TracefsCaptureSpec::function_graph(
            "catalog function graph",
            "mister-magik-catalog-graph",
            "/tmp/mister-magik/catalog-function-graph",
            GROUPS,
        );
        validate_tracefs_spec(spec).unwrap();
        let prepare = tracefs_prepare_command(spec);
        let start = tracefs_control_command(spec, true);
        let cleanup = tracefs_cleanup_command(spec);
        assert!(prepare.contains("available_tracers"));
        assert!(prepare.contains("available_filter_functions"));
        assert!(prepare.contains("function-group:directory-walk"));
        assert!(prepare.contains("function-group:durability"));
        assert!(prepare.contains("set_graph_function"));
        assert!(prepare.contains("max_graph_depth"));
        assert!(prepare.contains(&FUNCTION_GRAPH_MAX_DEPTH.to_string()));
        assert!(prepare.contains(&FUNCTION_GRAPH_BUFFER_KB.to_string()));
        assert!(start.contains("function_graph"));
        assert!(cleanup.contains("current_tracer"));
        assert!(cleanup.contains("printf 'nop"));
        assert!(cleanup.contains("rmdir"));
    }

    #[test]
    fn function_graph_parser_ranks_timed_functions() {
        let trace = r#"
 0)               |  iterate_dir() {
 0)   4.250 us    |  } /* iterate_dir */
 1)   1.500 us    |  vfs_fsync();
"#;
        let summary =
            summarize_function_graph_trace(trace, "overrun: 0\ncommit overrun: 0\n").unwrap();
        assert_eq!(summary["parsed_records"], 3);
        assert_eq!(summary["top_functions"][0]["function"], "iterate_dir");
        assert_eq!(summary["top_functions"][0]["timed_total_us"], 4);
        assert_eq!(summary["top_functions"][1]["function"], "vfs_fsync");
    }

    #[test]
    fn function_graph_spec_rejects_unbounded_or_unsafe_filters() {
        const EMPTY_GROUPS: &[TracefsFunctionGroup] = &[];
        const UNSAFE_GROUPS: &[TracefsFunctionGroup] = &[TracefsFunctionGroup {
            label: "unsafe",
            functions: &["vfs_*"],
        }];
        assert!(
            validate_tracefs_spec(TracefsCaptureSpec::function_graph(
                "empty",
                "mister-magik-empty",
                "/tmp/mister-magik/empty",
                EMPTY_GROUPS,
            ))
            .is_err()
        );
        assert!(
            validate_tracefs_spec(TracefsCaptureSpec::function_graph(
                "unsafe",
                "mister-magik-unsafe",
                "/tmp/mister-magik/unsafe",
                UNSAFE_GROUPS,
            ))
            .is_err()
        );
    }
}
