// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::tracefs::{
    SCHEDULER_TRACE_SPEC, STORAGE_TRACE_SPEC, TracefsCapture, summarize_scheduler_trace,
    summarize_storage_trace,
};
use super::*;
use std::collections::BTreeMap;
use std::fmt::Write as _;

const STORAGE_OUTPUT_ROOT: &str = "/media/fat/mister-magik-dev/storage-attribution-benchmark";
const STORAGE_WORK_ROOT: &str = "/tmp/mister-magik/storage-attribution-workload";
const STORAGE_TIMEOUT_TENTHS: u64 = 12_000;
const STORAGE_MAX_KIB: u64 = 512 * 1024;

pub(super) fn profile_installed_scheduler_trace(
    config: &NativeDeviceConfig,
    output_dir: &Path,
) -> Result<String> {
    fs::create_dir_all(output_dir)?;
    let session = connect_with(&config.connection, 30)?;
    let manifest = remote_read(&session, LOCAL_MAIN_MANIFEST_REMOTE)
        .ok_or("development manifest is unavailable before scheduler tracing")?;
    let installed_identity = streamline_installed_identity(&session, config.agent()?, &manifest)?;
    let original_reply = exec_checked_output(
        &session,
        "query scheduler trace display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    if parse_display_reply_pending(original_reply.stdout.trim())?.is_some() {
        return Err("scheduler trace cannot start during a display transaction".into());
    }
    let original_id = parse_display_reply_active(original_reply.stdout.trim())?;
    let original_mode = DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == original_id)
        .copied()
        .ok_or_else(|| format!("scheduler trace cannot restore unknown mode {original_id}"))?;
    let capture_mode = DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == "hdmi-1280x720p60")
        .copied()
        .ok_or("missing scheduler trace display mode")?;
    let capture = TracefsCapture::new(&session, output_dir, SCHEDULER_TRACE_SPEC);
    capture.prepare()?;
    let display_result = apply_confirmed_display_mode(config, capture_mode, "scheduler trace");
    if let Err(error) = display_result {
        let cleanup = capture.cleanup();
        return match cleanup {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!(
                "scheduler trace display setup failed: {error}; trace cleanup failed: {cleanup}"
            )
            .into()),
        };
    }

    let run_result = (|| -> Result<(Value, Value, String, String, String, String)> {
        capture.start()?;
        let route = run_gui_frame_profile_arm(config, &session, &output_dir.join("route"), false);
        let stop = capture.stop();
        let retained = stop.and_then(|()| capture.retain("scheduler-trace.txt"));
        let route = route?;
        let retained = retained?;
        let raw = fs::read_to_string(&retained.raw_path)?;
        let (scheduler, threads, cpus, irqs) = summarize_scheduler_trace(&raw, &retained.stats)?;
        fs::write(output_dir.join("thread-summary.tsv"), &threads)?;
        fs::write(output_dir.join("cpu-summary.tsv"), &cpus)?;
        fs::write(output_dir.join("irq-summary.tsv"), &irqs)?;
        let trace = json!({
            "path": "scheduler-trace.txt",
            "sha256": retained.sha256,
            "clock": "mono",
            "buffer_kb_per_cpu": SCHEDULER_TRACE_SPEC.buffer_kb,
            "capabilities": parse_trace_capabilities(&retained.capabilities),
            "stats": "trace-stats.txt",
        });
        Ok((
            route,
            scheduler,
            threads,
            cpus,
            irqs,
            serde_json::to_string(&trace)?,
        ))
    })();
    let trace_cleanup = capture.cleanup();
    let launcher_restore = launcher_restart(
        &session,
        &LauncherRestartOptions {
            clear_env: true,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.as_str().into(),
            timeout_secs: 45,
            ..LauncherRestartOptions::default()
        },
    );
    let route_cleanup = exec_checked(
        &session,
        "clean scheduler trace GUI route state",
        &gui_profile_route_cleanup_command(),
    );
    let display_restore =
        apply_confirmed_display_mode(config, original_mode, "scheduler trace display restoration");
    let (route, scheduler, _threads, _cpus, _irqs, trace) = match (
        run_result,
        trace_cleanup,
        launcher_restore,
        route_cleanup,
        display_restore,
    ) {
        (Ok(result), Ok(()), Ok(()), Ok(()), Ok(())) => result,
        (run, trace, launcher, route, display) => {
            return Err(format!(
                "scheduler trace failed: run={:?}; trace_cleanup={:?}; launcher_restore={:?}; route_cleanup={:?}; display_restore={:?}",
                run.err(),
                trace.err(),
                launcher.err(),
                route.err(),
                display.err()
            )
            .into());
        }
    };
    let final_manifest = remote_read(&session, LOCAL_MAIN_MANIFEST_REMOTE)
        .ok_or("development manifest is unavailable after scheduler tracing")?;
    if final_manifest != manifest {
        return Err("installed platform manifest changed during scheduler tracing".into());
    }
    let final_identity = streamline_installed_identity(&session, config.agent()?, &final_manifest)?;
    if final_identity != installed_identity {
        return Err("installed identity changed during scheduler tracing".into());
    }
    let trace: Value = serde_json::from_str(&trace)?;
    let summary = json!({
        "schema": "mister-magik-scheduler-trace-v1",
        "artifact_status": "passed",
        "product_quality_status": "not-applicable-attribution-only",
        "performance_authority": "diagnostic-observer",
        "identity": installed_identity_json(&installed_identity),
        "display_mode": capture_mode.id,
        "refresh_hz": 60,
        "workload": "fixed-gui-profile-route",
        "route": route,
        "trace": trace,
        "scheduler": scheduler,
        "artifacts": {
            "threads": "thread-summary.tsv",
            "cpus": "cpu-summary.tsv",
            "interrupts": "irq-summary.tsv",
            "capabilities": "trace-capabilities.tsv",
        },
    });
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    fs::write(output_dir.join("report.md"), scheduler_report(&summary)?)?;
    serde_json::to_string(&summary).map_err(Into::into)
}

pub(super) fn profile_installed_storage_attribution(
    config: &NativeDeviceConfig,
    output_dir: &Path,
) -> Result<String> {
    fs::create_dir_all(output_dir)?;
    let session = connect_with(&config.connection, 30)?;
    let manifest = remote_read(&session, LOCAL_MAIN_MANIFEST_REMOTE)
        .ok_or("development manifest is unavailable before storage attribution")?;
    let installed_identity = streamline_installed_identity(&session, config.agent()?, &manifest)?;
    let registry_before = exec_checked_output(
        &session,
        "read catalog registry before storage attribution",
        DEVELOPMENT_CATALOG_REGISTRY_REPORT_COMMAND.as_str(),
    )?;
    let mount = storage_mount_probe(&session)?;
    exec_checked(
        &session,
        "prepare isolated storage attribution root",
        &storage_prepare_command(),
    )?;
    let capture = TracefsCapture::new(&session, output_dir, STORAGE_TRACE_SPEC);
    if let Err(error) = capture.prepare() {
        let storage_cleanup = exec_checked(
            &session,
            "clean failed storage attribution preparation",
            &storage_cleanup_command(),
        );
        return match storage_cleanup {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!(
                "storage trace preparation failed: {error}; storage cleanup failed: {cleanup}"
            )
            .into()),
        };
    }

    let run_result = (|| -> Result<Value> {
        exec_checked(
            &session,
            "suspend launcher for isolated storage attribution",
            &acknowledged_main_command("mister_magik_suspend"),
        )?;
        capture.start()?;
        let connection = config.connection.clone();
        let command = storage_workload_command(&mount.block_device);
        let workload = thread::spawn(move || -> std::result::Result<ExecOutput, String> {
            let workload_session =
                connect_with(&connection, 30).map_err(|error| error.to_string())?;
            exec(&workload_session, &command, true).map_err(|error| error.to_string())
        });
        let workload_result = workload
            .join()
            .map_err(|_| "storage attribution workload thread panicked")?;
        let stop_result = capture.stop();
        let retained = stop_result.and_then(|()| capture.retain("storage-trace.txt"));
        let workload_output =
            workload_result.map_err(|error| format!("storage workload: {error}"))?;
        let retained = retained?;
        get(
            &session,
            &format!("{STORAGE_WORK_ROOT}/library-refresh.log"),
            &output_dir.join("library-refresh.log"),
        )?;
        get(
            &session,
            &format!("{STORAGE_WORK_ROOT}/process-io.tsv"),
            &output_dir.join("process-io.tsv"),
        )?;
        get(
            &session,
            &format!("{STORAGE_WORK_ROOT}/block-stat.tsv"),
            &output_dir.join("block-stat.tsv"),
        )?;
        if let Some(error) = exec_failure_message("isolated storage workload", &workload_output) {
            return Err(error.into());
        }
        let root_pid = remote_read(&session, &format!("{STORAGE_WORK_ROOT}/workload.pid"))
            .ok_or("storage attribution workload PID is missing")?
            .trim()
            .parse::<u32>()?;
        let inspect = exec_checked_output(
            &session,
            "inspect isolated storage attribution catalog",
            &storage_catalog_command("catalog-inspect"),
        )?;
        if !inspect.stdout.contains("catalog_v3_summary_tsv") || !inspect.stdout.contains("valid=1")
        {
            return Err("isolated storage attribution catalog is invalid".into());
        }
        let catalog_identity = parse_storage_catalog_identity(&inspect.stdout)?;
        fs::write(output_dir.join("catalog-inspect.tsv"), &inspect.stdout)?;
        let output_kib = exec_checked_output(
            &session,
            "measure isolated storage attribution output",
            &format!("du -sk {} | awk '{{print $1}}'", sh(STORAGE_OUTPUT_ROOT)),
        )?
        .stdout
        .trim()
        .parse::<u64>()?;
        let process_text = fs::read_to_string(output_dir.join("process-io.tsv"))?;
        let block_text = fs::read_to_string(output_dir.join("block-stat.tsv"))?;
        let process = summarize_process_io(&process_text)?;
        let block = summarize_block_stats(&block_text)?;
        write_json_lines(
            output_dir.join("process-io.jsonl"),
            process_text.lines().filter_map(parse_sample_line),
        )?;
        write_json_lines(
            output_dir.join("block-stat.jsonl"),
            block_text.lines().filter_map(parse_sample_line),
        )?;
        let raw_trace = fs::read_to_string(&retained.raw_path)?;
        let trace_summary = summarize_storage_trace(&raw_trace, &retained.stats, root_pid)?;
        let workload_log = fs::read_to_string(output_dir.join("library-refresh.log"))?;
        let phases = parse_storage_phase_markers(&workload_log);
        let namespace_arm = summarize_namespace_arm(&phases)?;
        let trace = json!({
            "path": "storage-trace.txt",
            "sha256": retained.sha256,
            "clock": "mono",
            "buffer_kb_per_cpu": STORAGE_TRACE_SPEC.buffer_kb,
            "capabilities": parse_trace_capabilities(&retained.capabilities),
            "stats": "trace-stats.txt",
            "summary": trace_summary,
        });
        Ok(json!({
            "schema": "mister-magik-storage-attribution-v1",
            "artifact_status": "passed",
            "product_quality_status": "not-applicable-attribution-only",
            "performance_authority": "diagnostic-observer",
            "identity": installed_identity_json(&installed_identity),
            "filesystem": mount.as_json(),
            "workload": {
                "command": "library-refresh",
                "source_policy": "normal configured library sources",
                "isolated_output_root": STORAGE_OUTPUT_ROOT,
                "output_kib": output_kib,
                "timeout_seconds": STORAGE_TIMEOUT_TENTHS / 10,
                "maximum_output_kib": STORAGE_MAX_KIB,
                "root_pid": root_pid,
            },
            "process_io": process,
            "block_io": block,
            "phase_markers": phases,
            "namespace_arm": namespace_arm,
            "catalog_identity": catalog_identity,
            "trace": trace,
            "artifacts": {
                "process_samples": "process-io.jsonl",
                "block_samples": "block-stat.jsonl",
                "workload_log": "library-refresh.log",
                "catalog_inspect": "catalog-inspect.tsv",
                "capabilities": "trace-capabilities.tsv",
            },
        }))
    })();
    let storage_cleanup = exec_checked(
        &session,
        "clean isolated storage attribution root",
        &storage_cleanup_command(),
    );
    let trace_cleanup = capture.cleanup();
    let launcher_restore = launcher_restart(
        &session,
        &LauncherRestartOptions {
            clear_env: true,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.as_str().into(),
            timeout_secs: 45,
            ..LauncherRestartOptions::default()
        },
    );
    let summary = match (run_result, storage_cleanup, trace_cleanup, launcher_restore) {
        (Ok(summary), Ok(()), Ok(()), Ok(())) => summary,
        (run, storage, trace, launcher) => {
            return Err(format!(
                "storage attribution failed: run={:?}; storage_cleanup={:?}; trace_cleanup={:?}; launcher_restore={:?}",
                run.err(),
                storage.err(),
                trace.err(),
                launcher.err()
            )
            .into());
        }
    };
    let final_manifest = remote_read(&session, LOCAL_MAIN_MANIFEST_REMOTE)
        .ok_or("development manifest is unavailable after storage attribution")?;
    if final_manifest != manifest {
        return Err("installed platform manifest changed during storage attribution".into());
    }
    let final_identity = streamline_installed_identity(&session, config.agent()?, &final_manifest)?;
    if final_identity != installed_identity {
        return Err("installed identity changed during storage attribution".into());
    }
    let registry_after = exec_checked_output(
        &session,
        "read catalog registry after storage attribution",
        DEVELOPMENT_CATALOG_REGISTRY_REPORT_COMMAND.as_str(),
    )?;
    if registry_after.stdout != registry_before.stdout {
        return Err("production catalog registry changed during storage attribution".into());
    }
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    fs::write(output_dir.join("report.md"), storage_report(&summary)?)?;
    serde_json::to_string(&summary).map_err(Into::into)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StorageMount {
    mount_point: String,
    filesystem: String,
    source: String,
    options: String,
    block_device: String,
}

impl StorageMount {
    fn as_json(&self) -> Value {
        json!({
            "mount_point": self.mount_point,
            "filesystem": self.filesystem,
            "source": self.source,
            "options": self.options,
            "block_device": self.block_device,
            "block_stat": format!("/sys/class/block/{}/stat", self.block_device),
        })
    }
}

fn storage_mount_probe(session: &Session) -> Result<StorageMount> {
    let output = exec_checked_output(
        session,
        "resolve exFAT backing block device",
        &storage_mount_probe_command(),
    )?;
    parse_storage_mount(output.stdout.trim())
}

fn storage_mount_probe_command() -> String {
    "set -eu; row=$(awk '$5 == \"/media/fat\" { for (i=1; i<=NF; i++) if ($i == \"-\") { print $(i+1) \"\\t\" $(i+2) \"\\t\" $6; exit } }' /proc/self/mountinfo); test -n \"$row\"; fstype=$(printf '%s\\n' \"$row\" | cut -f1); source=$(printf '%s\\n' \"$row\" | cut -f2); options=$(printf '%s\\n' \"$row\" | cut -f3); device=$(basename \"$source\"); block=; if test -e \"/sys/class/block/$device\"; then real=$(readlink -f \"/sys/class/block/$device\"); candidate=$(basename \"$(dirname \"$real\")\"); if test -f \"/sys/class/block/$candidate/stat\"; then block=$candidate; elif test -f \"/sys/class/block/$device/stat\"; then block=$device; fi; fi; if test -z \"$block\"; then set -- /sys/block/mmcblk*/stat; test \"$#\" -eq 1; block=$(basename \"$(dirname \"$1\")\"); fi; test -f \"/sys/class/block/$block/stat\"; printf 'mount=/media/fat\\tfstype=%s\\tsource=%s\\toptions=%s\\tblock=%s\\n' \"$fstype\" \"$source\" \"$options\" \"$block\"".to_owned()
}

fn parse_storage_mount(text: &str) -> Result<StorageMount> {
    let fields = parse_tab_fields(text);
    let mount = StorageMount {
        mount_point: required_field(&fields, "mount")?.to_owned(),
        filesystem: required_field(&fields, "fstype")?.to_owned(),
        source: required_field(&fields, "source")?.to_owned(),
        options: required_field(&fields, "options")?.to_owned(),
        block_device: required_field(&fields, "block")?.to_owned(),
    };
    if mount.mount_point != "/media/fat"
        || !mount.block_device.starts_with("mmcblk")
        || mount.filesystem.is_empty()
    {
        return Err(format!("unexpected storage mount attribution: {mount:?}").into());
    }
    Ok(mount)
}

fn parse_tab_fields(text: &str) -> BTreeMap<String, String> {
    text.trim()
        .split('\t')
        .filter_map(|field| field.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn required_field<'a>(fields: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str> {
    fields
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("storage attribution field {key} is missing").into())
}

fn storage_prepare_command() -> String {
    let safety = platform_safety_script();
    format!(
        "set -eu; rm -rf {output} {work}; mkdir -p {output} {work}; test -d {output}; test -d {work}; {safety}",
        output = sh(STORAGE_OUTPUT_ROOT),
        work = sh(STORAGE_WORK_ROOT),
    )
}

fn storage_catalog_command(subcommand: &str) -> String {
    format!(
        "env MISTER_SHARDED_CATALOG_DIR={catalog} MISTER_LIBRARY_SQLITE={library} MISTER_ARCADE_BOOTSTRAP_INDEX={bootstrap} MISTER_LIBRARY_REFRESH_LOCK={refresh_lock} MISTER_CATALOG_BUILDER_LOCK={builder_lock} MISTER_CATALOG_READY_SNAPSHOT={ready_snapshot} MISTER_CATALOG_DIAGNOSTICS_DIR={diagnostics} MISTER_LIBRARY_NAMESPACE_BACKEND=fd-relative MISTER_MAGIK_FOREGROUND_LIBRARY_REFRESH=1 {gui} {subcommand}",
        catalog = sh(&format!("{STORAGE_OUTPUT_ROOT}/catalog-v3")),
        library = sh(&format!("{STORAGE_OUTPUT_ROOT}/library.sqlite3")),
        bootstrap = sh(&format!("{STORAGE_OUTPUT_ROOT}/arcade-bootstrap.nav.lz4b")),
        refresh_lock = sh(&format!("{STORAGE_OUTPUT_ROOT}/library-refresh.lock")),
        builder_lock = sh(&format!("{STORAGE_OUTPUT_ROOT}/catalog-builder.lock")),
        ready_snapshot = sh(&format!("{STORAGE_OUTPUT_ROOT}/catalog-ready.snapshot")),
        diagnostics = sh(&format!("{STORAGE_OUTPUT_ROOT}/diagnostics")),
        gui = sh(DEVELOPMENT_GUI_REMOTE),
        subcommand = sh(subcommand),
    )
}

fn storage_workload_command(block_device: &str) -> String {
    let invocation = storage_catalog_command("library-refresh");
    let template = r#"set -eu
root=@OUTPUT@
work=@WORK@
gui=@GUI@
block_stat=@BLOCK_STAT@
process_file="$work/process-io.tsv"
block_file="$work/block-stat.tsv"
log="$work/library-refresh.log"
pid_file="$work/workload.pid"
: > "$process_file"
: > "$block_file"
sample_block() {
  ts=$(awk '{printf "%.0f", $1 * 1000000}' /proc/uptime)
  printf 'monotonic_us=%s' "$ts" >> "$block_file"
  index=0
  for value in $(cat "$block_stat"); do
    index=$((index+1))
    printf '\tf%s=%s' "$index" "$value" >> "$block_file"
  done
  printf '\n' >> "$block_file"
}
sample_process() {
  test -r "/proc/$pid/io" || return 0
  ts=$(awk '{printf "%.0f", $1 * 1000000}' /proc/uptime)
  printf 'monotonic_us=%s' "$ts" >> "$process_file"
  while read -r key value; do
    key=${key%:}
    printf '\t%s=%s' "$key" "$value" >> "$process_file"
  done < "/proc/$pid/io"
  voluntary=$(awk '$1 == "voluntary_ctxt_switches:" {print $2}' "/proc/$pid/status")
  involuntary=$(awk '$1 == "nonvoluntary_ctxt_switches:" {print $2}' "/proc/$pid/status")
  printf '\tvoluntary_ctxt_switches=%s\tnonvoluntary_ctxt_switches=%s\n' "${voluntary:-0}" "${involuntary:-0}" >> "$process_file"
}
terminate_owned() {
  exe=$(readlink "/proc/$pid/exe" 2>/dev/null || true)
  test -z "$exe" && return 0
  test "$exe" = "$gui"
  kill -TERM "$pid"
  tries=0
  while kill -0 "$pid" 2>/dev/null && test "$tries" -lt 100; do tries=$((tries+1)); sleep 0.1; done
  if kill -0 "$pid" 2>/dev/null; then kill -KILL "$pid"; fi
}
sample_block
@INVOCATION@ > "$log" 2>&1 &
pid=$!
tries=0
while test "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" != "$gui" && test "$tries" -lt 100; do
  kill -0 "$pid" 2>/dev/null || break
  tries=$((tries+1))
  sleep 0.01
done
test "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" = "$gui"
printf '%s\n' "$pid" > "$pid_file"
i=0
limit=
while kill -0 "$pid" 2>/dev/null; do
  sample_process || true
  sample_block
  kib=$(du -sk "$root" | awk '{print $1}')
  if test "$kib" -gt @MAX_KIB@; then limit=size; terminate_owned; break; fi
  i=$((i+1))
  if test "$i" -ge @TIMEOUT_TENTHS@; then limit=timeout; terminate_owned; break; fi
  sleep 0.1
done
set +e
wait "$pid"
rc=$?
set -e
sample_block
printf 'rc=%s\tlimit=%s\n' "$rc" "${limit:-none}" > "$work/workload-result.tsv"
test -z "$limit"
exit "$rc"
"#;
    template
        .replace("@OUTPUT@", &sh(STORAGE_OUTPUT_ROOT))
        .replace("@WORK@", &sh(STORAGE_WORK_ROOT))
        .replace("@GUI@", &sh(DEVELOPMENT_GUI_REMOTE))
        .replace(
            "@BLOCK_STAT@",
            &sh(&format!("/sys/class/block/{block_device}/stat")),
        )
        .replace("@INVOCATION@", &invocation)
        .replace("@MAX_KIB@", &STORAGE_MAX_KIB.to_string())
        .replace("@TIMEOUT_TENTHS@", &STORAGE_TIMEOUT_TENTHS.to_string())
}

fn storage_cleanup_command() -> String {
    let safety = platform_safety_script();
    let template = r#"set -eu
work=@WORK@
pid_file="$work/workload.pid"
if test -f "$pid_file"; then
  pid=$(cat "$pid_file")
  case "$pid" in ''|*[!0-9]*) exit 19;; esac
  exe=$(readlink "/proc/$pid/exe" 2>/dev/null || true)
  if test -n "$exe"; then
    test "$exe" = @GUI@
    kill -TERM "$pid"
    i=0
    while kill -0 "$pid" 2>/dev/null && test "$i" -lt 100; do i=$((i+1)); sleep 0.1; done
    if kill -0 "$pid" 2>/dev/null; then kill -KILL "$pid"; fi
    test ! -e "/proc/$pid/exe"
  fi
fi
rm -rf @OUTPUT@ @WORK@
test ! -e @OUTPUT@
test ! -e @WORK@
@SAFETY@
"#;
    template
        .replace("@WORK@", &sh(STORAGE_WORK_ROOT))
        .replace("@OUTPUT@", &sh(STORAGE_OUTPUT_ROOT))
        .replace("@GUI@", &sh(DEVELOPMENT_GUI_REMOTE))
        .replace("@SAFETY@", &safety)
}

fn parse_sample_line(line: &str) -> Option<Value> {
    let fields = parse_tab_fields(line);
    (!fields.is_empty()).then(|| json!(fields))
}

fn parse_samples(text: &str) -> Result<Vec<BTreeMap<String, String>>> {
    let samples = text
        .lines()
        .map(parse_tab_fields)
        .filter(|sample| !sample.is_empty())
        .collect::<Vec<_>>();
    if samples.len() < 2 {
        return Err("storage attribution requires at least two samples".into());
    }
    Ok(samples)
}

fn counter(sample: &BTreeMap<String, String>, key: &str) -> Result<u64> {
    required_field(sample, key)?
        .parse::<u64>()
        .map_err(Into::into)
}

fn counter_delta(
    first: &BTreeMap<String, String>,
    last: &BTreeMap<String, String>,
    key: &str,
) -> Result<u64> {
    Ok(counter(last, key)?.saturating_sub(counter(first, key)?))
}

fn summarize_process_io(text: &str) -> Result<Value> {
    let samples = parse_samples(text)?;
    let first = samples.first().expect("at least two");
    let last = samples.last().expect("at least two");
    let mut deltas = serde_json::Map::new();
    for key in [
        "rchar",
        "wchar",
        "syscr",
        "syscw",
        "read_bytes",
        "write_bytes",
        "cancelled_write_bytes",
        "voluntary_ctxt_switches",
        "nonvoluntary_ctxt_switches",
    ] {
        deltas.insert(key.to_owned(), json!(counter_delta(first, last, key)?));
    }
    Ok(json!({
        "samples": samples.len(),
        "duration_us": counter(last, "monotonic_us")?.saturating_sub(counter(first, "monotonic_us")?),
        "deltas": deltas,
    }))
}

fn summarize_block_stats(text: &str) -> Result<Value> {
    let samples = parse_samples(text)?;
    let fields = samples[0].keys().filter(|key| key.starts_with('f')).count();
    if !matches!(fields, 11 | 15 | 17) {
        return Err(format!("unsupported block-stat field count {fields}").into());
    }
    if samples
        .iter()
        .any(|sample| sample.keys().filter(|key| key.starts_with('f')).count() != fields)
    {
        return Err("block-stat field count changed during storage attribution".into());
    }
    let first = samples.first().expect("at least two");
    let last = samples.last().expect("at least two");
    let read_ios = counter_delta(first, last, "f1")?;
    let read_ticks = counter_delta(first, last, "f4")?;
    let write_ios = counter_delta(first, last, "f5")?;
    let write_ticks = counter_delta(first, last, "f8")?;
    let max_in_flight = samples
        .iter()
        .map(|sample| counter(sample, "f9"))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .max()
        .unwrap_or(0);
    Ok(json!({
        "samples": samples.len(),
        "field_count": fields,
        "duration_us": counter(last, "monotonic_us")?.saturating_sub(counter(first, "monotonic_us")?),
        "read_ios": read_ios,
        "read_sectors": counter_delta(first, last, "f3")?,
        "read_bytes": counter_delta(first, last, "f3")?.saturating_mul(512),
        "read_wait_ms": read_ticks,
        "read_average_wait_ms": if read_ios == 0 { 0.0 } else { read_ticks as f64 / read_ios as f64 },
        "write_ios": write_ios,
        "write_sectors": counter_delta(first, last, "f7")?,
        "write_bytes": counter_delta(first, last, "f7")?.saturating_mul(512),
        "write_wait_ms": write_ticks,
        "write_average_wait_ms": if write_ios == 0 { 0.0 } else { write_ticks as f64 / write_ios as f64 },
        "io_ticks_ms": counter_delta(first, last, "f10")?,
        "weighted_queue_ms": counter_delta(first, last, "f11")?,
        "max_in_flight": max_in_flight,
    }))
}

fn write_json_lines(path: PathBuf, values: impl IntoIterator<Item = Value>) -> Result<()> {
    let mut output = String::new();
    for value in values {
        writeln!(output, "{}", serde_json::to_string(&value)?)?;
    }
    fs::write(path, output)?;
    Ok(())
}

fn parse_storage_phase_markers(log: &str) -> Value {
    Value::Array(
        log.lines()
            .filter(|line| line.contains("_tsv") && line.contains('='))
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let event = fields.next()?.to_owned();
                let values = fields
                    .filter_map(|field| field.split_once('='))
                    .map(|(key, value)| (key.to_owned(), Value::String(value.to_owned())))
                    .collect::<serde_json::Map<_, _>>();
                Some(json!({"event": event, "fields": values}))
            })
            .collect(),
    )
}

fn parse_storage_catalog_identity(inspect: &str) -> Result<Value> {
    let summary = inspect
        .lines()
        .find(|line| line.starts_with("catalog_v3_summary_tsv\t"))
        .ok_or("storage catalog inspection omitted its summary row")?;
    let fields = summary
        .split('\t')
        .skip(1)
        .filter_map(|field| field.split_once('='))
        .collect::<BTreeMap<_, _>>();
    if fields.get("valid").copied() != Some("1") {
        return Err("storage catalog inspection is not valid".into());
    }
    let required = |key: &str| {
        fields
            .get(key)
            .copied()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("storage catalog inspection omitted {key}"))
    };
    Ok(json!({
        "valid": true,
        "systems": required("systems")?.parse::<u64>()?,
        "total_games": required("total_games")?.parse::<u64>()?,
        "fingerprint": required("fingerprint")?,
        "identity_sha256": required("identity_sha256")?,
        "ordering_sha256": required("ordering_sha256")?,
        "launch_sha256": required("launch_sha256")?,
        "search_sha256": required("search_sha256")?,
        "artifact_set_sha256": required("artifact_set_sha256")?,
    }))
}

fn summarize_namespace_arm(markers: &Value) -> Result<Value> {
    let markers = markers
        .as_array()
        .ok_or("storage phase markers are not an array")?;
    let marker = |event: &str| {
        markers
            .iter()
            .find(|marker| marker["event"].as_str() == Some(event))
            .and_then(|marker| marker["fields"].as_object())
    };
    let number = |fields: &serde_json::Map<String, Value>, key: &str| -> Result<u64> {
        fields
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("namespace attribution omitted {key}"))?
            .parse::<u64>()
            .map_err(Into::into)
    };
    let handoff = marker("catalog_scan_handoff_tsv")
        .ok_or("namespace arm omitted catalog scan handoff attribution")?;
    let attribution = marker("catalog_scan_attribution_tsv")
        .ok_or("namespace arm omitted catalog scan attribution")?;
    let handoffs = markers
        .iter()
        .filter(|marker| marker["event"] == "catalog_target_handoff_tsv")
        .filter_map(|marker| marker["fields"].as_object())
        .filter_map(|fields| {
            let ordinal = fields.get("ordinal")?.as_str()?.parse::<u64>().ok()?;
            Some((ordinal, fields))
        })
        .collect::<BTreeMap<_, _>>();
    let mut backend_counts = BTreeMap::<String, u64>::new();
    let mut targets = Vec::new();
    for fields in markers
        .iter()
        .filter(|marker| marker["event"] == "catalog_namespace_target_tsv")
        .filter_map(|marker| marker["fields"].as_object())
    {
        let ordinal = number(fields, "ordinal")?;
        let backend = fields
            .get("backend")
            .and_then(Value::as_str)
            .ok_or("namespace target omitted backend")?;
        *backend_counts.entry(backend.to_string()).or_default() += 1;
        let consumer = handoffs.get(&ordinal).copied();
        targets.push(json!({
            "ordinal": ordinal,
            "path": fields.get("path").and_then(Value::as_str).unwrap_or("unknown"),
            "backend": backend,
            "first_entry_us": number(fields, "first_entry_us")?,
            "final_entry_us": number(fields, "final_entry_us")?,
            "producer_complete_us": number(fields, "producer_complete_us")?,
            "consumer_first_work_us": consumer
                .and_then(|fields| fields.get("consumer_first_work_us"))
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
            "target_complete_us": consumer
                .and_then(|fields| fields.get("consumer_complete_us"))
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
            "buffered_entries": number(fields, "buffered_entries")?,
            "buffered_bytes": number(fields, "buffered_bytes")?,
            "buffer_allocations": number(fields, "buffer_allocations")?,
            "fallbacks": number(fields, "fallbacks")?,
            "restarts": number(fields, "restarts")?,
        }));
    }
    if targets.is_empty() {
        return Err("namespace arm produced no per-target attribution".into());
    }
    Ok(json!({
        "backend_selector": "fd-relative",
        "producer_us": number(attribution, "execution_producer_us")?,
        "channel_wait_us": number(attribution, "execution_send_us")?,
        "consumer_wait_us": number(handoff, "receive_wait_us")?,
        "consumer_active_us": number(handoff, "consumer_active_us")?,
        "maximum_buffered_entries": number(attribution, "execution_peak_buffered_entries")?,
        "maximum_buffered_bytes": number(attribution, "execution_peak_buffered_bytes")?,
        "buffer_allocations": number(attribution, "execution_buffer_allocations")?,
        "fallbacks": number(attribution, "execution_fallback_count")?,
        "restarts": number(attribution, "execution_restart_count")?,
        "backend_counts": backend_counts,
        "targets": targets,
    }))
}

fn storage_report(summary: &Value) -> Result<String> {
    let mut report = String::from("# exFAT storage attribution\n\n");
    writeln!(
        report,
        "Artifact: **{}**\n",
        summary["artifact_status"].as_str().unwrap_or("failed")
    )?;
    writeln!(
        report,
        "- Filesystem: `{}`",
        summary["filesystem"]["filesystem"]
            .as_str()
            .unwrap_or("unknown")
    )?;
    writeln!(
        report,
        "- Block device: `{}`",
        summary["filesystem"]["block_device"]
            .as_str()
            .unwrap_or("unknown")
    )?;
    writeln!(
        report,
        "- Process reads: {} bytes",
        summary["process_io"]["deltas"]["read_bytes"]
            .as_u64()
            .unwrap_or(0)
    )?;
    writeln!(
        report,
        "- Process writes: {} bytes",
        summary["process_io"]["deltas"]["write_bytes"]
            .as_u64()
            .unwrap_or(0)
    )?;
    writeln!(
        report,
        "- Block reads: {} bytes",
        summary["block_io"]["read_bytes"].as_u64().unwrap_or(0)
    )?;
    writeln!(
        report,
        "- Block writes: {} bytes",
        summary["block_io"]["write_bytes"].as_u64().unwrap_or(0)
    )?;
    writeln!(
        report,
        "- Maximum in-flight I/O: {}\n",
        summary["block_io"]["max_in_flight"].as_u64().unwrap_or(0)
    )?;
    writeln!(
        report,
        "- Namespace producer/channel wait: {} / {} us",
        summary["namespace_arm"]["producer_us"]
            .as_u64()
            .unwrap_or(0),
        summary["namespace_arm"]["channel_wait_us"]
            .as_u64()
            .unwrap_or(0)
    )?;
    writeln!(
        report,
        "- Namespace consumer wait/active: {} / {} us",
        summary["namespace_arm"]["consumer_wait_us"]
            .as_u64()
            .unwrap_or(0),
        summary["namespace_arm"]["consumer_active_us"]
            .as_u64()
            .unwrap_or(0)
    )?;
    writeln!(
        report,
        "- Namespace peak buffer: {} entries / {} bytes\n",
        summary["namespace_arm"]["maximum_buffered_entries"]
            .as_u64()
            .unwrap_or(0),
        summary["namespace_arm"]["maximum_buffered_bytes"]
            .as_u64()
            .unwrap_or(0)
    )?;
    report.push_str("This capture is diagnostic attribution only. All writable catalog paths were redirected to the isolated Dev benchmark root and removed after capture.\n");
    Ok(report)
}

fn installed_identity_json(identity: &StreamlineInstalledIdentity) -> Value {
    json!({
        "boot_id": identity.boot_id,
        "platform_manifest_sha256": identity.platform_manifest_sha256,
        "magik_revision": identity.magik_revision,
        "gui_sha256": identity.gui_sha256,
        "agent_sha256": identity.agent_sha256,
        "agent_bytes": identity.agent_bytes,
        "agent_version": identity.agent_version,
    })
}

fn parse_trace_capabilities(text: &str) -> Value {
    Value::Array(
        text.lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(event, status)| json!({"event": event, "status": status}))
            .collect(),
    )
}

fn scheduler_report(summary: &Value) -> Result<String> {
    let scheduler = &summary["scheduler"];
    let mut report = String::from("# Scheduler trace\n\n");
    writeln!(
        report,
        "Artifact: **{}**\n",
        summary["artifact_status"].as_str().unwrap_or("failed")
    )?;
    writeln!(
        report,
        "- Duration: {} us",
        scheduler["duration_us"].as_u64().unwrap_or(0)
    )?;
    writeln!(
        report,
        "- Dual-core overlap: {} us ({:.3}%)",
        scheduler["dual_core_overlap_us"].as_u64().unwrap_or(0),
        scheduler["dual_core_overlap_pct"].as_f64().unwrap_or(0.0)
    )?;
    writeln!(
        report,
        "- Parsed events: {}",
        scheduler["event_count"].as_u64().unwrap_or(0)
    )?;
    writeln!(
        report,
        "- Trace overruns: {}\n",
        scheduler["trace_overruns"].as_u64().unwrap_or(u64::MAX)
    )?;
    report.push_str("This capture is diagnostic attribution only; the unprofiled route remains product-quality authority.\n");
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_preserve_required_and_missing_status() {
        let capabilities = parse_trace_capabilities(
            "sched:sched_switch\trequired\nblock:block_rq_issue\tmissing\n",
        );
        assert_eq!(capabilities.as_array().map(Vec::len), Some(2));
        assert_eq!(capabilities[1]["status"], "missing");
    }

    #[test]
    fn scheduler_report_keeps_attribution_separate_from_quality() {
        let summary = json!({
            "artifact_status": "passed",
            "scheduler": {
                "duration_us": 1000,
                "dual_core_overlap_us": 250,
                "dual_core_overlap_pct": 25.0,
                "event_count": 42,
                "trace_overruns": 0,
            }
        });
        let report = scheduler_report(&summary).unwrap();
        assert!(report.contains("diagnostic attribution only"));
        assert!(report.contains("25.000%"));
    }

    #[test]
    fn storage_mount_requires_media_fat_and_mmc() {
        let mount = parse_storage_mount(
            "mount=/media/fat\tfstype=fuseblk\tsource=/dev/mmcblk0p1\toptions=rw\tblock=mmcblk0",
        )
        .unwrap();
        assert_eq!(mount.block_device, "mmcblk0");
        assert!(
            parse_storage_mount("mount=/tmp\tfstype=tmpfs\tsource=tmpfs\toptions=rw\tblock=sda")
                .is_err()
        );
    }

    #[test]
    fn process_io_summary_reports_counter_deltas() {
        let samples = "monotonic_us=100\trchar=10\twchar=20\tsyscr=1\tsyscw=2\tread_bytes=0\twrite_bytes=4096\tcancelled_write_bytes=0\tvoluntary_ctxt_switches=3\tnonvoluntary_ctxt_switches=1\nmonotonic_us=300\trchar=110\twchar=220\tsyscr=11\tsyscw=22\tread_bytes=8192\twrite_bytes=12288\tcancelled_write_bytes=0\tvoluntary_ctxt_switches=13\tnonvoluntary_ctxt_switches=4\n";
        let summary = summarize_process_io(samples).unwrap();
        assert_eq!(summary["duration_us"], 200);
        assert_eq!(summary["deltas"]["read_bytes"], 8_192);
        assert_eq!(summary["deltas"]["write_bytes"], 8_192);
        assert_eq!(summary["deltas"]["nonvoluntary_ctxt_switches"], 3);
    }

    #[test]
    fn block_summary_accepts_legacy_and_current_field_counts() {
        for fields in [11usize, 15, 17] {
            let first = (1..=fields)
                .map(|index| format!("f{index}={}", if index == 9 { 1 } else { 10 }))
                .collect::<Vec<_>>()
                .join("\t");
            let last = (1..=fields)
                .map(|index| format!("f{index}={}", if index == 9 { 4 } else { 20 }))
                .collect::<Vec<_>>()
                .join("\t");
            let summary = summarize_block_stats(&format!(
                "monotonic_us=100\t{first}\nmonotonic_us=200\t{last}\n"
            ))
            .unwrap();
            assert_eq!(summary["field_count"], fields);
            assert_eq!(summary["max_in_flight"], 4);
        }
    }

    #[test]
    fn storage_commands_are_fixed_bounded_and_owned() {
        let workload = storage_workload_command("mmcblk0");
        let cleanup = storage_cleanup_command();
        assert!(workload.contains(STORAGE_OUTPUT_ROOT));
        assert!(workload.contains(&STORAGE_TIMEOUT_TENTHS.to_string()));
        assert!(workload.contains(&STORAGE_MAX_KIB.to_string()));
        assert!(workload.contains("MISTER_LIBRARY_NAMESPACE_BACKEND=fd-relative"));
        assert!(workload.contains("/proc/$pid/exe"));
        assert!(cleanup.contains("test \"$exe\" ="));
        assert!(cleanup.contains(STORAGE_OUTPUT_ROOT));
        assert!(!workload.contains("/dev/mmcblk0"));
    }

    #[test]
    fn storage_phase_markers_keep_structured_tsv_fields() {
        let markers = parse_storage_phase_markers(
            "noise\ncatalog_phase_tsv phase=scan elapsed_us=42 files=9\n",
        );
        assert_eq!(markers.as_array().map(Vec::len), Some(1));
        assert_eq!(markers[0]["event"], "catalog_phase_tsv");
        assert_eq!(markers[0]["fields"]["elapsed_us"], "42");
    }

    #[test]
    fn namespace_arm_joins_producer_and_consumer_target_timing() {
        let markers = parse_storage_phase_markers(
            "catalog_scan_handoff_tsv receive_wait_us=30 consumer_active_us=40\n\
             catalog_scan_attribution_tsv execution_producer_us=10 execution_send_us=20 execution_peak_buffered_entries=7 execution_peak_buffered_bytes=700 execution_buffer_allocations=3 execution_fallback_count=1 execution_restart_count=1\n\
             catalog_namespace_target_tsv ordinal=2 first_entry_us=5 final_entry_us=8 producer_complete_us=9 buffered_entries=7 buffered_bytes=700 buffer_allocations=3 fallbacks=1 restarts=1 backend=walkdir-fallback path=/games/test\n\
             catalog_target_handoff_tsv ordinal=2 consumer_first_work_us=6 consumer_complete_us=12\n",
        );
        let summary = summarize_namespace_arm(&markers).unwrap();

        assert_eq!(summary["producer_us"], 10);
        assert_eq!(summary["consumer_active_us"], 40);
        assert_eq!(summary["maximum_buffered_entries"], 7);
        assert_eq!(summary["targets"][0]["consumer_first_work_us"], 6);
        assert_eq!(summary["targets"][0]["target_complete_us"], 12);
        assert_eq!(summary["targets"][0]["restarts"], 1);
    }

    #[test]
    fn storage_catalog_identity_requires_all_behavior_hashes() {
        let identity = parse_storage_catalog_identity(
            "catalog_v3_summary_tsv\tvalid=1\tsystems=2\ttotal_games=3\tfingerprint=f\tidentity_sha256=i\tordering_sha256=o\tlaunch_sha256=l\tsearch_sha256=s\tartifact_set_sha256=a\n",
        )
        .unwrap();

        assert_eq!(identity["systems"], 2);
        assert_eq!(identity["ordering_sha256"], "o");
        assert_eq!(identity["artifact_set_sha256"], "a");
    }
}
