// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::io::BufWriter;

const CONTROL_REMOTE: &str = "/tmp/mister-magik/latch-v5-qualification-control.tsv";
const CONTROL_TEMP_REMOTE: &str = "/tmp/mister-magik/latch-v5-qualification-control.tsv.tmp";
const STATE_REMOTE: &str = "/tmp/mister-magik/latch-v5-qualification-state.json";
const CATALOG_STATE_REMOTE: &str = "/tmp/mister-magik/latch-v5-catalog";
const DURATION: Duration = Duration::from_secs(6 * 60 * 60);
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const PROGRESS_INTERVAL: Duration = Duration::from_secs(5 * 60);
const STRESS_CLASS_INTERVAL: Duration = Duration::from_secs(5 * 60);
const CATALOG_REQUEST_INTERVAL: Duration = Duration::from_secs(7 * 60 + 30);
const MAX_CATALOG_REQUESTS: u64 = 48;
const MIN_CATALOG_GENERATIONS: u64 = 12;
const MIN_ACCEPTED_CONFIRMED_FRAMES: u64 = 1_000_000;
const MIN_CATALOG_OVERLAP_FRAMES: u64 = 250_000;
const MIN_STRESS_CLASS_FRAMES: u64 = 25_000;
const MIN_RECEIPT_STATUS_SAMPLES: u64 = 4_000;
const MAX_RSS_GROWTH_KB: u64 = 32 * 1024;
const MAX_RSS_HWM_KB: u64 = 192 * 1024;
const MAX_FRAME_WALL_US: u64 = 250_000;
const MAX_VSYNC_MISS_STREAK: u64 = 2;
const STRESS_CLASSES: [&str; 6] = [
    "particles",
    "transitions",
    "arcade-scroll",
    "preview-archive",
    "search-filter-model",
    "input-traffic",
];

pub(super) fn run(config: &NativeDeviceConfig) -> Result<String> {
    let preflight_session = connect_with(&config.connection, 10)?;
    exec_checked(
        &preflight_session,
        "latch v5 qualification preflight",
        &format!("set -eu; test -s {RELEASE_TOKEN}"),
    )?;
    let development = exec(
        &preflight_session,
        "pidof MiSTer_MagiKDev >/dev/null 2>&1",
        false,
    )?
    .rc == 0;
    let root = if development {
        installed_layout::paths(Layout::Development).root
    } else {
        installed_layout::paths(Layout::Public).root
    };
    let remote_env = if development {
        DEVELOPMENT_LAUNCHER_ENV_REMOTE.as_str()
    } else {
        DEFAULT_LAUNCHER_ENV_REMOTE.as_str()
    };
    let qualification_env = qualification_launcher_env();
    put_bytes(
        &preflight_session,
        remote_env,
        one_shot_launcher_env_text(&qualification_env, remote_env).as_bytes(),
    )?;
    issue_reboot(&preflight_session, RebootMode::Supervised)?;
    drop(preflight_session);
    if !wait_down_with(&config.connection, 40.0) || wait_up_with(&config.connection, 120.0)? != 0 {
        return Err("latch v5 qualification clean boot did not complete".into());
    }

    let session = connect_with(&config.connection, 10)?;
    exec_checked(
        &session,
        "latch v5 qualification rearm token",
        &release_rearm_token_command(),
    )?;
    let current_development =
        exec(&session, "pidof MiSTer_MagiKDev >/dev/null 2>&1", false)?.rc == 0;
    if current_development != development {
        return Err("Main layout changed across the qualification clean boot".into());
    }
    exec_checked(
        &session,
        "latch v5 qualification prepare",
        &format!(
            "set -eu; test -s {RELEASE_TOKEN}; rm -f {remote_env}; mkdir -p /tmp/mister-magik; rm -f {CONTROL_TEMP_REMOTE}; mkdir -p {CATALOG_STATE_REMOTE}"
        ),
    )?;
    write_control(&session, STRESS_CLASSES[0], 1)?;

    let initial_state = wait_qualification_state(&session, Duration::from_secs(45))?;
    let baseline_identity = required_pointer(&initial_state, "/identity")?.clone();
    validate_identity(&baseline_identity)?;
    let baseline_status = read_launcher_status(&session)?;
    let baseline_main_status = read_main_status(&session)?;
    let baseline_main_ownership = main_qualification_baseline(&baseline_main_status)?;
    let baseline_rss_kb = u64_at(&baseline_status, "/rss_kb")?;
    let output_dir = qualification_output_dir(&baseline_identity)?;
    fs::create_dir_all(&output_dir)?;
    let raw_path = output_dir.join("samples.ndjson");
    let mut raw = BufWriter::new(fs::File::create(&raw_path)?);
    let started = Instant::now();
    let mut next_sample = started;
    let mut next_progress = started + PROGRESS_INTERVAL;
    let mut samples = 0u64;
    let mut last_state = initial_state;
    let mut peak_rss_hwm_kb = u64_at(&baseline_status, "/rss_hwm_kb")?;
    let run_result = (|| -> Result<()> {
        while started.elapsed() < DURATION {
            let now = Instant::now();
            if now < next_sample {
                thread::sleep(next_sample - now);
            }
            let elapsed = started.elapsed();
            let class_index = ((elapsed.as_secs() / STRESS_CLASS_INTERVAL.as_secs()) as usize)
                % STRESS_CLASSES.len();
            let catalog_request = (elapsed.as_secs() / CATALOG_REQUEST_INTERVAL.as_secs() + 1)
                .min(MAX_CATALOG_REQUESTS);
            write_control(&session, STRESS_CLASSES[class_index], catalog_request)?;
            let sample = collect_sample(
                &session,
                root,
                elapsed,
                STRESS_CLASSES[class_index],
                catalog_request,
            )?;
            peak_rss_hwm_kb = peak_rss_hwm_kb.max(u64_at(
                required_pointer(&sample, "/status")?,
                "/rss_hwm_kb",
            )?);
            last_state = required_pointer(&sample, "/qualification_state")?.clone();
            serde_json::to_writer(&mut raw, &sample)?;
            raw.write_all(b"\n")?;
            raw.flush()?;
            samples = samples.saturating_add(1);
            validate_sample(
                &sample,
                &baseline_identity,
                &baseline_main_ownership,
                root,
                &session,
            )?;
            if Instant::now() >= next_progress {
                println!(
                    "latch-v5 qualification elapsed={}m samples={} accepted={} overlap={} catalogs={}",
                    elapsed.as_secs() / 60,
                    samples,
                    u64_at(&last_state, "/accepted_confirmed_frames")?,
                    u64_at(&last_state, "/catalog_overlap_frames")?,
                    u64_at(&last_state, "/catalog_completed")?
                );
                next_progress += PROGRESS_INTERVAL;
            }
            next_sample += SAMPLE_INTERVAL;
        }
        validate_final(
            &last_state,
            samples,
            baseline_rss_kb,
            peak_rss_hwm_kb,
            &read_launcher_status(&session)?,
        )
    })();
    raw.flush()?;
    let raw_bytes = fs::read(&raw_path)?;
    let raw_sha256 = encode_hex(&Sha256::digest(&raw_bytes));
    let summary = json!({
        "schema": "mister-magik-latch-v5-qualification-summary-v1",
        "qualified": run_result.is_ok(),
        "failure": run_result.as_ref().err().map(ToString::to_string),
        "duration_required_secs": DURATION.as_secs(),
        "sample_interval_secs": SAMPLE_INTERVAL.as_secs(),
        "samples": samples,
        "identity": baseline_identity,
        "main_ownership_baseline": baseline_main_ownership,
        "final_state": last_state,
        "baseline_rss_kb": baseline_rss_kb,
        "peak_rss_hwm_kb": peak_rss_hwm_kb,
        "raw_evidence": {
            "path": raw_path,
            "sha256": raw_sha256,
            "bytes": raw_bytes.len(),
        },
    });
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    run_result?;
    Ok(format!(
        "latch-v5-qualified evidence={}",
        output_dir.display()
    ))
}

fn qualification_launcher_env() -> Vec<(String, String)> {
    vec![
        ("MISTER_LATCH_V5_QUALIFICATION".into(), "1".into()),
        ("MISTER_LAUNCHER_START_SCREEN".into(), "arcade".into()),
        (
            "MISTER_LIBRARY_SQLITE".into(),
            format!("{CATALOG_STATE_REMOTE}/library.sqlite3"),
        ),
        (
            "MISTER_LIBRARY_SQLITE_BUILD_DIR".into(),
            format!("{CATALOG_STATE_REMOTE}/sqlite-build"),
        ),
        (
            "MISTER_SHARDED_CATALOG_DIR".into(),
            format!("{CATALOG_STATE_REMOTE}/catalog-v3"),
        ),
        (
            "MISTER_CATALOG_READY_SNAPSHOT".into(),
            format!("{CATALOG_STATE_REMOTE}/catalog-ready.nav.lz4b"),
        ),
        (
            "MISTER_CATALOG_BUILDER_LOCK".into(),
            format!("{CATALOG_STATE_REMOTE}/catalog-builder.lock"),
        ),
    ]
}

fn write_control(session: &Session, stress_class: &str, catalog_request: u64) -> Result<()> {
    let text = format!(
        "schema=mister-magik-latch-v5-qualification-control-v1 stress_class={stress_class} catalog_request={catalog_request}\n"
    );
    put_bytes(session, CONTROL_TEMP_REMOTE, text.as_bytes())?;
    exec_checked(
        session,
        "latch v5 qualification control",
        &format!("set -eu; mv {CONTROL_TEMP_REMOTE} {CONTROL_REMOTE}"),
    )
}

fn wait_qualification_state(session: &Session, timeout: Duration) -> Result<Value> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Some(text) = remote_read(session, STATE_REMOTE)
            && let Ok(state) = serde_json::from_str::<Value>(&text)
            && qualification_state_ready(&state)
        {
            return Ok(state);
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err("latch v5 qualification state did not become ready".into())
}

fn qualification_state_ready(state: &Value) -> bool {
    state.get("schema").and_then(Value::as_str)
        == Some("mister-magik-latch-v5-qualification-state-v1")
        && state.get("control_error").is_some_and(Value::is_null)
        && state
            .get("catalog_requested")
            .and_then(Value::as_u64)
            .is_some_and(|generation| generation >= 1)
}

fn collect_sample(
    session: &Session,
    root: &str,
    elapsed: Duration,
    stress_class: &str,
    catalog_request: u64,
) -> Result<Value> {
    let qualification_state_text =
        remote_read(session, STATE_REMOTE).ok_or("latch v5 qualification state disappeared")?;
    let qualification_state = serde_json::from_str::<Value>(&qualification_state_text)?;
    let status = read_launcher_status(session)?;
    let main_status = read_main_status(session)?;
    let readiness_output = exec(
        session,
        &format!("{root}/mister-magik-fb latch-readiness-report --json"),
        true,
    )?;
    if let Some(error) = exec_failure_message("latch v5 readiness sample", &readiness_output) {
        return Err(error.into());
    }
    let readiness = readiness_output
        .stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Value>(line).ok())
        .ok_or("latch readiness sample did not contain JSON")?;
    let latch_output = exec(
        session,
        &format!("{root}/mister-magik-fb fpga-latch-report"),
        true,
    )?;
    if let Some(error) = exec_failure_message("latch v5 authoritative sample", &latch_output) {
        return Err(error.into());
    }
    Ok(json!({
        "schema": "mister-magik-latch-v5-qualification-sample-v1",
        "elapsed_ms": elapsed.as_millis() as u64,
        "requested_stress_class": stress_class,
        "requested_catalog_generation": catalog_request,
        "qualification_state": qualification_state,
        "status": status,
        "main_status": main_status,
        "readiness": readiness,
        "latch_report": latch_output.stdout,
    }))
}

fn validate_sample(
    sample: &Value,
    baseline_identity: &Value,
    baseline_main_ownership: &Value,
    root: &str,
    session: &Session,
) -> Result<()> {
    let state = required_pointer(sample, "/qualification_state")?;
    if required_pointer(state, "/identity")? != baseline_identity {
        return Err("qualification identity changed during the run".into());
    }
    if !state.get("control_error").is_none_or(Value::is_null) {
        return Err(format!("qualification control failed: {}", state["control_error"]).into());
    }
    let status = required_pointer(sample, "/status")?;
    require_text(status, "/scene", "launcher")?;
    require_text(status, "/present_backend", "fpga-vblank-latch-hidden")?;
    require_text(status, "/present_status", "ok")?;
    require_bool(status, "/display_frozen", false)?;
    if !matches!(
        status.get("latch_failure_state").and_then(Value::as_str),
        None | Some("") | Some("none")
    ) {
        return Err(format!(
            "runtime recorded a latch failure: {}",
            status["latch_failure_state"]
        )
        .into());
    }
    let main = required_pointer(sample, "/main_status")?;
    validate_main_qualification_sample(main, baseline_main_ownership)?;
    require_text(required_pointer(sample, "/readiness")?, "/state", "ready")?;
    let latch = sample
        .get("latch_report")
        .and_then(Value::as_str)
        .ok_or("latch report is missing")?;
    for required in [
        "production_ready=1",
        "protocol_version=5",
        "flags=0x03ff",
        "supported=1",
        "drop_count=0",
    ] {
        if !latch.contains(required) {
            return Err(format!("latch report is missing {required}").into());
        }
    }
    if latch.contains("supported=0") {
        return Err("latch report contains an unsupported v5 operation".into());
    }
    let namespace = state
        .get("identity_namespace")
        .and_then(Value::as_str)
        .ok_or("qualification identity namespace is missing")?;
    if namespace.contains("..") || namespace.starts_with('/') {
        return Err("qualification identity namespace is unsafe".into());
    }
    let report = format!("{root}/diagnostics/latch/{namespace}/latest.json");
    if remote_read(session, &report).is_some() {
        return Err(
            format!("current qualification identity created a latch failure: {report}").into(),
        );
    }
    let frame_budget = required_pointer(status, "/frame_budget")?;
    if u64_at(frame_budget, "/max_wall_us")? > MAX_FRAME_WALL_US {
        return Err("frame latency exceeded the qualification bound".into());
    }
    if u64_at(frame_budget, "/max_vsync_miss_streak")? > MAX_VSYNC_MISS_STREAK {
        return Err("vblank miss streak exceeded the qualification bound".into());
    }
    Ok(())
}

fn read_main_status(session: &Session) -> Result<Value> {
    let text = remote_read(session, MAIN_STATUS_REMOTE).ok_or("Main status disappeared")?;
    Ok(serde_json::from_str(&text)?)
}

fn main_qualification_baseline(main: &Value) -> Result<Value> {
    require_text(main, "/launcher_state", "LauncherActive")?;
    require_text(main, "/fpga_owner", "magik")?;
    for pointer in ["/crash_count", "/invariant_count"] {
        let value = u64_at(main, pointer)?;
        if value != 0 {
            return Err(
                format!("Main qualification baseline is not clean: {pointer}={value}").into(),
            );
        }
    }

    Ok(json!({
        "main_generation": u64_at(main, "/main_generation")?,
        "pid": u64_at(main, "/pid")?,
        "fpga_owner_epoch": u64_at(main, "/fpga_owner_epoch")?,
        "blocked_spi_writes": u64_at(main, "/blocked_spi_writes")?,
        "blocked_gpo_writes": u64_at(main, "/blocked_gpo_writes")?,
        "last_blocked_fpga_site": main
            .get("last_blocked_fpga_site")
            .and_then(Value::as_str)
            .unwrap_or(""),
    }))
}

fn validate_main_qualification_sample(main: &Value, baseline: &Value) -> Result<()> {
    require_text(main, "/launcher_state", "LauncherActive")?;
    require_text(main, "/fpga_owner", "magik")?;
    for pointer in ["/main_generation", "/pid", "/fpga_owner_epoch"] {
        let expected = u64_at(baseline, pointer)?;
        let observed = u64_at(main, pointer)?;
        if observed != expected {
            return Err(format!(
                "Main changed during latch qualification: {pointer} baseline={expected} observed={observed}"
            )
            .into());
        }
    }
    for pointer in ["/blocked_spi_writes", "/blocked_gpo_writes"] {
        let initial = u64_at(baseline, pointer)?;
        let observed = u64_at(main, pointer)?;
        if observed != initial {
            return Err(format!(
                "Main blocked-write counter changed during latch qualification: {pointer} baseline={initial} observed={observed} owner_epoch={} last_blocked_site={}",
                u64_at(main, "/fpga_owner_epoch")?,
                main.get("last_blocked_fpga_site")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            )
            .into());
        }
    }
    for pointer in ["/crash_count", "/invariant_count"] {
        let observed = u64_at(main, pointer)?;
        if observed != 0 {
            return Err(
                format!("Main qualification counter is nonzero: {pointer}={observed}").into(),
            );
        }
    }
    Ok(())
}

fn validate_identity(identity: &Value) -> Result<()> {
    if matches!(
        identity.get("classification").and_then(Value::as_str),
        Some("mixed-invalid" | "unknown") | None
    ) {
        return Err("qualification identity is unknown or mixed".into());
    }
    require_text(identity, "/platform/latch_protocol_version", "5")
        .or_else(|_| require_u64(identity, "/platform/latch_protocol_version", 5))?;
    require_text(identity, "/platform/latch_capability_mask", "0x03ff")?;
    for pointer in [
        "/runtime/build_number",
        "/runtime/source_revision",
        "/runtime/binary_sha256",
        "/platform/release_tag",
        "/platform/bundle_id",
        "/platform/qualification_candidate_id",
        "/platform/manifest_sha256",
        "/platform/main_sha256",
        "/platform/scanout_module_sha256",
        "/platform/latch_rbf_sha256",
        "/device_boot_id",
        "/launcher_session_id",
    ] {
        let value = identity
            .pointer(pointer)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("qualification identity is missing {pointer}"))?;
        if value == "unknown" {
            return Err(format!("qualification identity is unknown at {pointer}").into());
        }
    }
    Ok(())
}

fn validate_final(
    state: &Value,
    samples: u64,
    baseline_rss_kb: u64,
    peak_rss_hwm_kb: u64,
    final_status: &Value,
) -> Result<()> {
    if u64_at(state, "/elapsed_ms")? < DURATION.as_millis() as u64 {
        return Err("qualification ended before six hours elapsed".into());
    }
    if u64_at(state, "/catalog_completed")? < MIN_CATALOG_GENERATIONS {
        return Err("qualification completed fewer than 12 cold catalog generations".into());
    }
    if u64_at(state, "/accepted_confirmed_frames")? < MIN_ACCEPTED_CONFIRMED_FRAMES {
        return Err("qualification confirmed fewer than 1,000,000 latch frames".into());
    }
    if u64_at(state, "/catalog_overlap_frames")? < MIN_CATALOG_OVERLAP_FRAMES {
        return Err("qualification recorded fewer than 250,000 catalog/UI overlap frames".into());
    }
    for key in [
        "particles",
        "transitions",
        "arcade_scroll",
        "preview_archive",
        "search_filter_model",
        "input_traffic",
    ] {
        if u64_at(state, &format!("/stress_class_frames/{key}"))? < MIN_STRESS_CLASS_FRAMES {
            return Err(format!("qualification stress class {key} has too few frames").into());
        }
    }
    if samples < MIN_RECEIPT_STATUS_SAMPLES {
        return Err("qualification collected fewer than 4,000 receipt/status samples".into());
    }
    if peak_rss_hwm_kb > MAX_RSS_HWM_KB {
        return Err("qualification exceeded the RSS high-water bound".into());
    }
    let final_rss_kb = u64_at(final_status, "/rss_kb")?;
    if final_rss_kb > baseline_rss_kb.saturating_add(MAX_RSS_GROWTH_KB) {
        return Err("qualification exceeded the RSS growth bound".into());
    }
    Ok(())
}

fn qualification_output_dir(identity: &Value) -> Result<PathBuf> {
    let candidate = identity
        .pointer("/platform/qualification_candidate_id")
        .and_then(Value::as_str)
        .ok_or("qualification candidate id is missing")?;
    let safe_candidate: String = candidate
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .take(64)
        .collect();
    if safe_candidate.is_empty() {
        return Err("qualification candidate id is unsafe".into());
    }
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    Ok(std::env::current_dir()?
        .join("build/release-qualification/latch-v5")
        .join(safe_candidate)
        .join(timestamp.to_string()))
}

fn required_pointer<'a>(value: &'a Value, pointer: &str) -> Result<&'a Value> {
    value
        .pointer(pointer)
        .ok_or_else(|| format!("qualification evidence is missing {pointer}").into())
}

fn u64_at(value: &Value, pointer: &str) -> Result<u64> {
    required_pointer(value, pointer)?
        .as_u64()
        .ok_or_else(|| format!("qualification evidence is not a u64 at {pointer}").into())
}

fn require_text(value: &Value, pointer: &str, expected: &str) -> Result<()> {
    let actual = required_pointer(value, pointer)?
        .as_str()
        .ok_or_else(|| format!("qualification evidence is not text at {pointer}"))?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!("qualification expected {pointer}={expected}, observed {actual}").into())
    }
}

fn require_u64(value: &Value, pointer: &str, expected: u64) -> Result<()> {
    let actual = u64_at(value, pointer)?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!("qualification expected {pointer}={expected}, observed {actual}").into())
    }
}

fn require_bool(value: &Value, pointer: &str, expected: bool) -> Result<()> {
    let actual = required_pointer(value, pointer)?
        .as_bool()
        .ok_or_else(|| format!("qualification evidence is not a bool at {pointer}"))?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!("qualification expected {pointer}={expected}, observed {actual}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_duration_and_sample_floor_are_mathematically_compatible() {
        assert_eq!(DURATION.as_secs(), 21_600);
        assert_eq!(SAMPLE_INTERVAL.as_secs(), 5);
        assert!(DURATION.as_secs() / SAMPLE_INTERVAL.as_secs() >= MIN_RECEIPT_STATUS_SAMPLES);
    }

    #[test]
    fn clean_boot_environment_is_isolated_and_self_contained() {
        let environment = qualification_launcher_env();
        for key in [
            "MISTER_LATCH_V5_QUALIFICATION",
            "MISTER_LIBRARY_SQLITE",
            "MISTER_LIBRARY_SQLITE_BUILD_DIR",
            "MISTER_SHARDED_CATALOG_DIR",
            "MISTER_CATALOG_READY_SNAPSHOT",
            "MISTER_CATALOG_BUILDER_LOCK",
        ] {
            assert!(environment.iter().any(|(candidate, _)| candidate == key));
        }
        for (_, value) in environment
            .iter()
            .filter(|(key, _)| key.starts_with("MISTER_CATALOG") || key.contains("SQLITE"))
        {
            assert!(value.starts_with(CATALOG_STATE_REMOTE));
        }
    }

    #[test]
    fn initial_state_waits_for_control_acknowledgement() {
        let mut state = json!({
            "schema": "mister-magik-latch-v5-qualification-state-v1",
            "control_error": "No such file or directory",
            "catalog_requested": 0,
        });
        assert!(!qualification_state_ready(&state));
        state["control_error"] = Value::Null;
        assert!(!qualification_state_ready(&state));
        state["catalog_requested"] = json!(1);
        assert!(qualification_state_ready(&state));
    }

    #[test]
    fn final_gate_enforces_every_documented_count() {
        let state = json!({
            "elapsed_ms": 21_600_000u64,
            "catalog_completed": 12,
            "accepted_confirmed_frames": 1_000_000,
            "catalog_overlap_frames": 250_000,
            "stress_class_frames": {
                "particles": 25_000,
                "transitions": 25_000,
                "arcade_scroll": 25_000,
                "preview_archive": 25_000,
                "search_filter_model": 25_000,
                "input_traffic": 25_000,
            }
        });
        let status = json!({"rss_kb": 32_000});
        validate_final(&state, 4_000, 16_000, 64_000, &status).unwrap();
        for pointer in [
            "/catalog_completed",
            "/accepted_confirmed_frames",
            "/catalog_overlap_frames",
        ] {
            let mut invalid = state.clone();
            *invalid.pointer_mut(pointer).unwrap() = json!(0);
            assert!(validate_final(&invalid, 4_000, 16_000, 64_000, &status).is_err());
        }
        assert!(validate_final(&state, 3_999, 16_000, 64_000, &status).is_err());
    }

    #[test]
    fn blocked_main_writes_are_gated_from_the_acknowledged_session_baseline() {
        let mut main = json!({
            "launcher_state": "LauncherActive",
            "fpga_owner": "magik",
            "main_generation": 9478,
            "pid": 722,
            "fpga_owner_epoch": 1,
            "blocked_spi_writes": 0,
            "blocked_gpo_writes": 3434,
            "last_blocked_fpga_site": "fpga_gpo_write",
            "crash_count": 0,
            "invariant_count": 0,
        });
        let baseline = main_qualification_baseline(&main).unwrap();
        validate_main_qualification_sample(&main, &baseline).unwrap();

        main["blocked_gpo_writes"] = json!(3435);
        assert!(validate_main_qualification_sample(&main, &baseline).is_err());
        main["blocked_gpo_writes"] = json!(3434);
        main["fpga_owner_epoch"] = json!(2);
        assert!(validate_main_qualification_sample(&main, &baseline).is_err());
    }

    #[test]
    fn identity_requires_every_release_and_runtime_component() {
        let identity = json!({
            "classification": "qualified",
            "runtime": {
                "build_number": "17",
                "source_revision": "runtime-rev",
                "binary_sha256": "runtime-sha",
            },
            "platform": {
                "latch_protocol_version": 5,
                "latch_capability_mask": "0x03ff",
                "release_tag": "platform-v0.7",
                "bundle_id": "bundle",
                "qualification_candidate_id": "candidate",
                "manifest_sha256": "manifest",
                "main_sha256": "main",
                "scanout_module_sha256": "module",
                "latch_rbf_sha256": "rbf",
            },
            "device_boot_id": "boot",
            "launcher_session_id": "session",
        });
        validate_identity(&identity).unwrap();

        let mut unknown = identity.clone();
        unknown["runtime"]["source_revision"] = json!("unknown");
        assert!(validate_identity(&unknown).is_err());
        let mut mixed = identity.clone();
        mixed["classification"] = json!("mixed-invalid");
        assert!(validate_identity(&mixed).is_err());
        let mut wrong_protocol = identity;
        wrong_protocol["platform"]["latch_protocol_version"] = json!(3);
        assert!(validate_identity(&wrong_protocol).is_err());
    }

    #[test]
    fn baseline_rejects_dirty_or_unowned_main_state() {
        let clean = json!({
            "launcher_state": "LauncherActive",
            "fpga_owner": "magik",
            "main_generation": 1,
            "pid": 2,
            "fpga_owner_epoch": 3,
            "blocked_spi_writes": 4,
            "blocked_gpo_writes": 5,
            "crash_count": 0,
            "invariant_count": 0,
        });
        assert!(main_qualification_baseline(&clean).is_ok());
        for (field, value) in [
            ("launcher_state", json!("Unconfigured")),
            ("fpga_owner", json!("main")),
            ("crash_count", json!(1)),
            ("invariant_count", json!(1)),
        ] {
            let mut invalid = clean.clone();
            invalid[field] = value;
            assert!(main_qualification_baseline(&invalid).is_err());
        }
    }

    #[test]
    fn final_gate_rejects_memory_and_each_stress_floor() {
        let state = json!({
            "elapsed_ms": DURATION.as_millis() as u64,
            "catalog_completed": MIN_CATALOG_GENERATIONS,
            "accepted_confirmed_frames": MIN_ACCEPTED_CONFIRMED_FRAMES,
            "catalog_overlap_frames": MIN_CATALOG_OVERLAP_FRAMES,
            "stress_class_frames": {
                "particles": MIN_STRESS_CLASS_FRAMES,
                "transitions": MIN_STRESS_CLASS_FRAMES,
                "arcade_scroll": MIN_STRESS_CLASS_FRAMES,
                "preview_archive": MIN_STRESS_CLASS_FRAMES,
                "search_filter_model": MIN_STRESS_CLASS_FRAMES,
                "input_traffic": MIN_STRESS_CLASS_FRAMES,
            }
        });
        let status = json!({"rss_kb": 10_000});
        assert!(validate_final(&state, MIN_RECEIPT_STATUS_SAMPLES, 8_000, 10_000, &status).is_ok());
        for key in [
            "particles",
            "transitions",
            "arcade_scroll",
            "preview_archive",
            "search_filter_model",
            "input_traffic",
        ] {
            let mut invalid = state.clone();
            invalid["stress_class_frames"][key] = json!(MIN_STRESS_CLASS_FRAMES - 1);
            assert!(
                validate_final(&invalid, MIN_RECEIPT_STATUS_SAMPLES, 8_000, 10_000, &status)
                    .is_err()
            );
        }
        assert!(
            validate_final(
                &state,
                MIN_RECEIPT_STATUS_SAMPLES,
                8_000,
                MAX_RSS_HWM_KB + 1,
                &status
            )
            .is_err()
        );
        let grown = json!({"rss_kb": 8_000 + MAX_RSS_GROWTH_KB + 1});
        assert!(validate_final(&state, MIN_RECEIPT_STATUS_SAMPLES, 8_000, 10_000, &grown).is_err());
    }
}
