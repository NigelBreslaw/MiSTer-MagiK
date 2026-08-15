// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::{
    DEVELOPMENT_GUI_REMOTE, IniEdit, MenuOutputProfile, RebootMode, Result,
    acknowledged_main_command, connect, crt_trial_run_command, edit_remote_ini, exec, exec_checked,
    exec_checked_output, issue_reboot, parse_crt_runtime_settings_reply, parse_crt_trial_status,
    platform_safety_script, remote_write, wait_down, wait_launcher_ready, wait_up,
};
use serde_json::{Value, json};
use ssh2::Session;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const REMOTE_INI: &str = "/media/fat/MiSTer.ini";
const REMOTE_BACKUP: &str = "/media/fat/MiSTer.ini.mister-magik-crt-backup";
const REMOTE_JOURNAL: &str = "/media/fat/MiSTer.ini.mister-magik-crt-journal-v1";
const REMOTE_RESTORE_NEW: &str = "/media/fat/MiSTer.ini.mister-magik-crt-restore-new";
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
unsafe extern "C" {
    fn signal(signal: i32, handler: extern "C" fn(i32)) -> usize;
}
#[cfg(unix)]
extern "C" fn note_interruption(_signal: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CrtMode {
    output: &'static str,
    profile: MenuOutputProfile,
    expected_rate: &'static str,
}

const CRT_MODES: [CrtMode; 4] = [
    CrtMode {
        output: "crt-240p60",
        profile: MenuOutputProfile::Crt240p60,
        expected_rate: "15.734 kHz / 60.052 Hz",
    },
    CrtMode {
        output: "crt-288p50",
        profile: MenuOutputProfile::Crt288p50,
        expected_rate: "15.734 kHz / 50.429 Hz",
    },
    CrtMode {
        output: "crt-480p60",
        profile: MenuOutputProfile::Crt480p60,
        expected_rate: "31.469 kHz / 59.940 Hz",
    },
    CrtMode {
        output: "crt-576p50",
        profile: MenuOutputProfile::Crt576p50,
        expected_rate: "31.469 kHz / 50.431 Hz",
    },
];

#[derive(Debug, Eq, PartialEq)]
enum QualifyAction {
    Attended {
        output: Option<PathBuf>,
    },
    Probe {
        pattern: String,
        seconds: u64,
        output: PathBuf,
    },
    Restore,
}

#[derive(Debug)]
struct OriginalState {
    main: String,
    output: String,
}

pub(super) fn run(args: &[String]) -> Result<()> {
    match parse_args(args)? {
        QualifyAction::Restore => restore_from_journal(),
        QualifyAction::Attended { output } => run_attended(output),
        QualifyAction::Probe {
            pattern,
            seconds,
            output,
        } => run_probe(&pattern, seconds, &output),
    }
}

fn parse_args(args: &[String]) -> Result<QualifyAction> {
    if args.first().map(String::as_str) == Some("probe") {
        return parse_probe_args(args);
    }
    if args.first().map(String::as_str) != Some("qualify") {
        return Err("usage: scripts/agent device crt <qualify|probe|restore> --attended".into());
    }
    if args.get(1).map(String::as_str) == Some("--restore") && args.len() == 2 {
        return Ok(QualifyAction::Restore);
    }
    if args.get(1).map(String::as_str) != Some("--attended") {
        return Err("CRT qualification requires --attended or --restore".into());
    }
    let mut output = None;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--out" => {
                let path = args.get(index + 1).ok_or("--out needs DIRECTORY")?;
                if output.replace(PathBuf::from(path)).is_some() {
                    return Err("--out may be specified only once".into());
                }
                index += 2;
            }
            other => return Err(format!("unsupported CRT qualification argument: {other}").into()),
        }
    }
    Ok(QualifyAction::Attended { output })
}

fn parse_probe_args(args: &[String]) -> Result<QualifyAction> {
    if args.get(1).map(String::as_str) != Some("--attended") {
        return Err("CRT probe requires --attended".into());
    }
    let mut pattern = None;
    let mut seconds = None;
    let mut output = None;
    let mut index = 2;
    while index < args.len() {
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("{} needs a value", args[index]))?;
        match args[index].as_str() {
            "--pattern" => {
                if pattern.replace(value.clone()).is_some() {
                    return Err("--pattern may be specified only once".into());
                }
            }
            "--seconds" => {
                if seconds.replace(value.parse::<u64>()?).is_some() {
                    return Err("--seconds may be specified only once".into());
                }
            }
            "--out" => {
                if output.replace(PathBuf::from(value)).is_some() {
                    return Err("--out may be specified only once".into());
                }
            }
            other => return Err(format!("unsupported CRT probe argument: {other}").into()),
        }
        index += 2;
    }
    let pattern = pattern.ok_or("CRT probe needs --pattern")?;
    if !matches!(
        pattern.as_str(),
        "fixed-a"
            | "fixed-b"
            | "identical-flip"
            | "slow-ab"
            | "full-ab"
            | "full-ab-hold2"
            | "full-ab-hold3"
            | "full-ab-hold4"
            | "motion"
            | "motion-hold2"
            | "motion-hold3"
            | "motion-slow"
            | "motion-color"
            | "preloaded-ruler-slow"
            | "preloaded-bars-slow"
    ) {
        return Err(format!("unsupported CRT probe pattern: {pattern}").into());
    }
    let seconds = seconds.ok_or("CRT probe needs --seconds 20")?;
    if seconds != 20 {
        return Err("CRT probe duration is fixed at 20 seconds".into());
    }
    let output = output.ok_or("CRT probe needs --out DIRECTORY")?;
    Ok(QualifyAction::Probe {
        pattern,
        seconds,
        output,
    })
}

fn run_probe(pattern: &str, seconds: u64, output: &Path) -> Result<()> {
    if !io::stdin().is_terminal() {
        return Err("CRT probe is attended and requires an interactive terminal".into());
    }
    create_new_output_directory(output)?;
    let session = connect(10)?;
    ensure_arming_clear(&session)?;
    ensure_no_existing_transaction(&session)?;
    let display = exec_checked_output(
        &session,
        "CRT probe display transaction preflight",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    if display
        .stdout
        .split_ascii_whitespace()
        .any(|field| field.starts_with("pending=") && field != "pending=none")
    {
        return Err("CRT probe refuses a pending display transaction".into());
    }
    wait_launcher_ready(&session, Instant::now(), Duration::from_secs(15))?;
    let settings = exec_checked_output(
        &session,
        "resolved CRT mode",
        &acknowledged_main_command("mister_magik_settings_get_v1"),
    )?;
    let runtime_settings = parse_crt_runtime_settings_reply(&settings.stdout)?;

    println!("CRT probe pattern={pattern} duration={seconds}s");
    println!("{}", probe_observation_prompt(pattern));
    exec_checked(
        &session,
        "CRT probe suspend",
        &acknowledged_main_command("mister_magik_suspend"),
    )?;
    let run = exec_checked(
        &session,
        "CRT probe",
        &crt_probe_run_command(pattern, seconds, &runtime_settings),
    );
    if let Err(error) = run {
        let recovery = exec_checked(
            &session,
            "CRT probe compensating resume",
            &acknowledged_main_command("mister_magik_resume"),
        );
        return match recovery {
            Ok(()) => Err(error),
            Err(recovery_error) => {
                Err(format!("{error}; compensating Main resume failed: {recovery_error}").into())
            }
        };
    }
    wait_launcher_ready(&session, Instant::now(), Duration::from_secs(15))?;
    let log = exec_checked_output(
        &session,
        "CRT probe log",
        "cat /tmp/mister-magik-crt_probe.log",
    )?;
    let status = parse_crt_probe_status(&log.stdout)?;
    fs::write(output.join("probe.log"), &log.stdout)?;
    fs::write(output.join("probe-status.txt"), format!("{status}\n"))?;
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "mister-magik-crt-probe-v1",
            "pattern": pattern,
            "seconds": seconds,
            "runtime_settings": runtime_settings,
            "status": status,
            "manual_observation_pending": true,
        }))?,
    )?;
    println!("{status}");
    println!("CRT probe restored launcher; report the physical CRT observation.");
    Ok(())
}

fn crt_probe_run_command(pattern: &str, seconds: u64, runtime_settings: &str) -> String {
    let resume = acknowledged_main_command("mister_magik_resume");
    format!(
        "cleanup() {{ trap - EXIT HUP INT TERM; {resume}; }}; trap cleanup EXIT HUP INT TERM; set -eu; test -x {gui}; MISTER_CRT_PROBE_PATTERN={} MISTER_MAGIK_RUNTIME_SETTINGS_V1={} {gui} ui crt_probe {seconds} >/tmp/mister-magik-crt_probe.log 2>&1",
        super::sh(pattern),
        super::sh(runtime_settings),
        gui = super::sh(DEVELOPMENT_GUI_REMOTE),
    )
}

fn probe_observation_prompt(pattern: &str) -> &'static str {
    match pattern {
        "fixed-a" => {
            "Observe whether slot A remains completely stable without ghosting or horizontal displacement."
        }
        "fixed-b" => {
            "Observe whether slot B remains completely stable without ghosting or horizontal displacement."
        }
        "identical-flip" => {
            "Both slots are identical; report any instability caused solely by full-rate base switching."
        }
        "slow-ab" => {
            "Report whether each cyan/magenta transition affects the whole raster or leaves a mismatched lower band."
        }
        "full-ab" => {
            "Report whether top and bottom identity colors agree and whether the 24-pixel displacement affects the whole raster."
        }
        "full-ab-hold2" => {
            "Each A/B grid is held for two rasters; report whether the lower-band displacement remains continuous, becomes transition-only, or disappears."
        }
        "full-ab-hold3" => {
            "Each A/B grid is held for three rasters; report whether the lower-band displacement remains continuous, becomes transition-only, or disappears."
        }
        "full-ab-hold4" => {
            "Each A/B grid is held for four rasters; report whether the lower-band displacement remains continuous, becomes transition-only, or disappears."
        }
        "motion" => "Report any lower-band ghost, horizontal step, or top/bottom frame mismatch.",
        "motion-hold2" => {
            "The bright ruler advances every two rasters. Report only whether you see the obvious lower-screen ghost or horizontal break."
        }
        "motion-hold3" => {
            "The bright ruler advances every three rasters. Report only whether you see the obvious lower-screen ghost or horizontal break."
        }
        "motion-slow" => {
            "The bright ruler steps once per second. At each step, report whether the lower screen briefly shows the old position, and whether it then becomes stable."
        }
        "motion-color" => {
            "A bar moves 12 pixels every raster and cycles red, cyan, yellow, blue, magenta, green. Report the current top/bottom band color and any remnant color."
        }
        "preloaded-ruler-slow" => {
            "Two ruler positions were preloaded before observation. At each one-second switch, report whether the lower screen briefly remains at the old position."
        }
        "preloaded-bars-slow" => {
            "Preloaded cyan-left and magenta-right bars switch once per second. Report any frame with one bar above a horizontal boundary and the other below it."
        }
        _ => "Observe the physical CRT.",
    }
}

fn parse_crt_probe_status(output: &str) -> Result<&str> {
    let status = output
        .match_indices("crt_probe_status_v1 ")
        .map(|(offset, _)| offset)
        .last()
        .map(|offset| &output[offset..])
        .unwrap_or(output)
        .lines()
        .next()
        .unwrap_or_default()
        .trim();
    if !status.starts_with("crt_probe_status_v1 schema=1 ") {
        return Err("CRT probe did not return a typed status response".into());
    }
    if status.split_ascii_whitespace().any(|field| field == "ok=0") {
        return Err(format!("CRT probe reported failure: {status}").into());
    }
    for required in [
        "ok=1",
        "pattern=",
        "mode=crt-",
        "duration_ms=",
        "slot_a_base=",
        "slot_b_base=",
        "writes=",
        "posts=",
        "flips=",
        "drops=0",
        "final_pending=0",
        "final_active_matches=1",
        "unsafe_active_writes=0",
        "pending_writes=0",
        "reason=none",
    ] {
        if !status
            .split_ascii_whitespace()
            .any(|field| field.starts_with(required))
        {
            return Err(format!("CRT probe status omitted successful {required}").into());
        }
    }
    Ok(status)
}

fn run_attended(output: Option<PathBuf>) -> Result<()> {
    if !io::stdin().is_terminal() {
        return Err("CRT qualification is attended and requires an interactive terminal".into());
    }
    let interrupted = install_interrupt_flag();
    let output = output.unwrap_or_else(default_output_directory);
    create_new_output_directory(&output)?;

    println!("CRT qualification artifacts: {}", output.display());
    println!("Capture: agent-cli native USB Video 1920x1080 JPEG");
    qualify_preflight_capture(&output, interrupted)?;

    let session = connect(10)?;
    ensure_arming_clear(&session)?;
    ensure_no_existing_transaction(&session)?;
    let original_ini = remote_read_bytes(&session, REMOTE_INI)?;
    let original = read_resolved_state(&session)?;
    begin_transaction(&session, &original_ini, &original, &output)?;
    drop(session);

    let trial_result = run_mode_matrix(&output, interrupted);
    let restore_result = restore_transaction(Some(&original));
    match (trial_result, restore_result) {
        (Ok(all_runtime_passed), Ok(())) => {
            fs::write(
                output.join("summary.json"),
                serde_json::to_vec_pretty(&json!({
                    "schema": 1,
                    "kind": "mister-magik-morph-functional-qualification",
                    "all_modes_runtime_passed": all_runtime_passed,
                    "all_modes_passed": false,
                    "visual_review_pending": true,
                    "real_crt_qualified": false,
                }))?,
            )?;
            println!(
                "CRT capture complete: all_modes_runtime_passed={all_runtime_passed} visual_review_pending=true real_crt_qualified=false"
            );
            if all_runtime_passed {
                Ok(())
            } else {
                Err("one or more CRT modes failed runtime qualification".into())
            }
        }
        (Err(trial), Ok(())) => Err(trial),
        (Ok(_), Err(restore)) => Err(format!(
            "CRT trials completed but automatic restore failed: {restore}; run `scripts/agent device crt restore --attended`"
        )
        .into()),
        (Err(trial), Err(restore)) => Err(format!(
            "CRT qualification failed: {trial}; restore also failed: {restore}; run `scripts/agent device crt restore --attended`"
        )
        .into()),
    }
}

fn install_interrupt_flag() -> &'static AtomicBool {
    INTERRUPTED.store(false, Ordering::SeqCst);
    #[cfg(unix)]
    // SAFETY: these handlers only set a lock-free atomic flag, which is
    // async-signal-safe. The host tool supports Unix hosts (macOS/Linux).
    unsafe {
        const SIGHUP: i32 = 1;
        const SIGINT: i32 = 2;
        const SIGTERM: i32 = 15;
        for signal_number in [SIGHUP, SIGINT, SIGTERM] {
            signal(signal_number, note_interruption);
        }
    }
    &INTERRUPTED
}

fn default_output_directory() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    std::env::temp_dir().join(format!("mister-magik-crt-{timestamp}"))
}

fn create_new_output_directory(output: &Path) -> Result<()> {
    if output.exists() {
        return Err(format!(
            "refusing existing CRT artifact directory: {}",
            output.display()
        )
        .into());
    }
    fs::create_dir_all(output)?;
    Ok(())
}

fn qualify_preflight_capture(output: &Path, interrupted: &AtomicBool) -> Result<()> {
    check_interrupted(interrupted)?;
    let path = output.join("preflight.jpg");
    capture_usb_video_frame(&path)?;
    println!("Preflight capture ready: {}", path.display());
    Ok(())
}

fn ensure_no_existing_transaction(session: &Session) -> Result<()> {
    exec_checked(
        session,
        "CRT qualification transaction preflight",
        &format!("set -eu; test ! -e {REMOTE_BACKUP}; test ! -e {REMOTE_JOURNAL}"),
    )
    .map_err(|_| {
        "unfinished CRT qualification transaction exists; run `scripts/agent device crt restore --attended`".into()
    })
}

fn ensure_arming_clear(session: &Session) -> Result<()> {
    exec_checked(
        session,
        "CRT qualification arming preflight",
        &format!("set -eu; {}", platform_safety_script()),
    )
}

fn begin_transaction(
    session: &Session,
    original_ini: &[u8],
    original: &OriginalState,
    output: &Path,
) -> Result<()> {
    remote_write(session, REMOTE_BACKUP, original_ini)?;
    let journal = serde_json::to_vec_pretty(&json!({
        "schema": 1,
        "original_main": original.main,
        "original_output": original.output,
        "artifact_directory": output.display().to_string(),
    }))?;
    let journal_result = remote_write(session, REMOTE_JOURNAL, &journal)
        .and_then(|()| exec_checked(session, "CRT qualification transaction sync", "sync"));
    if let Err(error) = journal_result {
        let _ = exec(
            session,
            &format!("rm -f {REMOTE_BACKUP} {REMOTE_JOURNAL} {REMOTE_RESTORE_NEW}; sync"),
            true,
        );
        return Err(error);
    }
    Ok(())
}

fn run_mode_matrix(output: &Path, interrupted: &AtomicBool) -> Result<bool> {
    let mut all_passed = true;
    for mode in CRT_MODES {
        check_interrupted(interrupted)?;
        let resolved = apply_mode(mode)?;
        let mode_directory = output.join(mode.output);
        fs::create_dir(&mode_directory)?;
        let (direct_video, menu_pal, forced_scandoubler) = mode.profile.settings();
        fs::write(
            mode_directory.join("expected.json"),
            serde_json::to_vec_pretty(&json!({
                "output": mode.output,
                "direct_video": direct_video,
                "menu_pal": menu_pal,
                "forced_scandoubler": forced_scandoubler,
                "expected_rate": mode.expected_rate,
            }))?,
        )?;
        fs::write(
            mode_directory.join("resolved.json"),
            serde_json::to_vec_pretty(&json!({
                "main": resolved.main,
                "output": resolved.output,
            }))?,
        )?;

        capture_analyzer(&mode_directory, mode, interrupted)?;
        let passed = run_mode_attempts(&mode_directory, mode, interrupted)?;
        all_passed &= passed;
    }
    Ok(all_passed)
}

fn apply_mode(mode: CrtMode) -> Result<OriginalState> {
    let session = connect(10)?;
    edit_remote_ini(&session, IniEdit::MenuOutput(mode.profile), false)?;
    issue_reboot(&session, RebootMode::Supervised)?;
    drop(session);
    if !wait_down(40.0) || wait_up(120.0)? != 0 {
        return Err(format!("{} did not complete its bounded reboot", mode.output).into());
    }
    let session = connect(10)?;
    wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))?;
    let resolved = read_resolved_state(&session)?;
    validate_resolved_mode(&resolved, mode)?;
    Ok(resolved)
}

fn validate_resolved_mode(resolved: &OriginalState, mode: CrtMode) -> Result<()> {
    if resolved.output != mode.output {
        return Err(format!(
            "{} resolved as {} after reboot",
            mode.output, resolved.output
        )
        .into());
    }
    Ok(())
}

fn capture_analyzer(directory: &Path, mode: CrtMode, interrupted: &AtomicBool) -> Result<()> {
    check_interrupted(interrupted)?;
    let path = directory.join("analyzer.jpg");
    capture_usb_video_frame(&path)?;
    println!(
        "Morph detected-input capture ready for {}: {}",
        mode.output,
        path.display()
    );
    Ok(())
}

fn run_mode_attempts(directory: &Path, mode: CrtMode, interrupted: &AtomicBool) -> Result<bool> {
    check_interrupted(interrupted)?;
    let attempt_directory = directory.join("attempt-01");
    fs::create_dir(&attempt_directory)?;
    let trial = run_crt_trial_once(mode, &attempt_directory);
    capture_usb_video_frame(&attempt_directory.join("trial.jpg"))?;
    let trial_status = match trial {
        Ok(status) => status,
        Err(error) => {
            fs::write(attempt_directory.join("trial-error.txt"), error.to_string())?;
            return Err(error);
        }
    };
    fs::write(attempt_directory.join("trial-status.txt"), &trial_status)?;
    fs::write(
        attempt_directory.join("verdict.json"),
        serde_json::to_vec_pretty(&json!({
            "mode": mode.output,
            "attempt": 1,
            "runtime_passed": true,
            "verdict": "visual-review-pending",
            "real_crt_qualified": false,
        }))?,
    )?;
    println!("Review artifacts: {}", attempt_directory.display());
    Ok(true)
}

fn run_crt_trial_once(mode: CrtMode, artifact_directory: &Path) -> Result<String> {
    let session = connect(10)?;
    let runtime_settings = format!("schema=1&output={}", mode.output);
    exec_checked(
        &session,
        "scene suspend",
        &acknowledged_main_command("mister_magik_suspend"),
    )?;
    let trial_result = exec_checked(
        &session,
        "operator CRT trial",
        &crt_trial_run_command(&runtime_settings, None), // Qualification never applies an override.
    );
    let log = exec_checked_output(
        &session,
        "CRT trial log snapshot",
        "if test -e /tmp/mister-magik-crt_trial.log; then cat /tmp/mister-magik-crt_trial.log; fi",
    )?;
    fs::write(artifact_directory.join("trial.log"), &log.stdout)?;
    trial_result?;
    let status = parse_crt_trial_status(&log.stdout)?;
    validate_trial_progress(status, mode)?;
    Ok(status.to_string())
}

fn validate_trial_progress(status: &str, mode: CrtMode) -> Result<()> {
    let field = |name: &str| {
        status
            .split_ascii_whitespace()
            .find_map(|field| field.strip_prefix(&format!("{name}=")))
    };
    if field("mode") != Some(mode.output) {
        return Err(format!("CRT trial status did not report {}", mode.output).into());
    }
    let mut values = [0_u64; 2];
    for (index, name) in ["frames", "flips"].into_iter().enumerate() {
        let value = field(name)
            .ok_or_else(|| format!("CRT trial status omitted {name}"))?
            .parse::<u64>()?;
        if value == 0 {
            return Err(format!("CRT trial status reported no advancing {name}").into());
        }
        values[index] = value;
    }
    if values[0] != values[1] {
        return Err(format!(
            "CRT trial left a latch flip incomplete: frames={} flips={}",
            values[0], values[1]
        )
        .into());
    }
    Ok(())
}

fn restore_from_journal() -> Result<()> {
    restore_transaction(None)
}

fn restore_transaction(expected: Option<&OriginalState>) -> Result<()> {
    let session = connect(10)?;
    let backup = remote_read_bytes(&session, REMOTE_BACKUP)?;
    let journal = remote_read_bytes(&session, REMOTE_JOURNAL)?;
    let journal: Value = serde_json::from_slice(&journal)?;
    if journal.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err("unsupported CRT qualification recovery journal".into());
    }
    let journal_expected = OriginalState {
        main: journal
            .get("original_main")
            .and_then(Value::as_str)
            .ok_or("CRT recovery journal omitted original_main")?
            .to_string(),
        output: journal
            .get("original_output")
            .and_then(Value::as_str)
            .ok_or("CRT recovery journal omitted original_output")?
            .to_string(),
    };
    if let Some(expected) = expected
        && (expected.main != journal_expected.main || expected.output != journal_expected.output)
    {
        return Err("CRT recovery journal does not match the active session".into());
    }
    remote_write(&session, REMOTE_RESTORE_NEW, &backup)?;
    exec_checked(
        &session,
        "restore original MiSTer.ini",
        &format!("set -eu; mv {REMOTE_RESTORE_NEW} {REMOTE_INI}; sync"),
    )?;
    issue_reboot(&session, RebootMode::Supervised)?;
    drop(session);
    if !wait_down(40.0) || wait_up(120.0)? != 0 {
        return Err("restored MiSTer.ini but bounded reboot did not complete".into());
    }
    let session = connect(10)?;
    wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))?;
    let restored = read_resolved_state(&session)?;
    if restored.main != journal_expected.main || restored.output != journal_expected.output {
        return Err(format!(
            "restored INI but runtime state differs: expected {}/{} got {}/{}",
            journal_expected.main, journal_expected.output, restored.main, restored.output
        )
        .into());
    }
    exec_checked(
        &session,
        "clear CRT qualification recovery journal",
        &format!("rm -f {REMOTE_BACKUP} {REMOTE_JOURNAL} {REMOTE_RESTORE_NEW}; sync"),
    )?;
    println!("Original MiSTer.ini, Main, and video mode restored");
    Ok(())
}

fn read_resolved_state(session: &Session) -> Result<OriginalState> {
    let main = exec_checked_output(
        session,
        "running MagiK Main",
        "if pidof MiSTer_MagiKDev >/dev/null 2>&1; then echo MiSTer_MagiKDev; elif pidof MiSTer_MagiK >/dev/null 2>&1; then echo MiSTer_MagiK; else exit 1; fi",
    )?
    .stdout
    .trim()
    .to_string();
    exec_checked_output(session, "running MagiK launcher", "pidof mister-magik-fb")?;
    Ok(OriginalState {
        main,
        output: "unverified".to_string(),
    })
}

fn remote_read_bytes(session: &Session, remote: &str) -> Result<Vec<u8>> {
    let sftp = session.sftp()?;
    let mut file = sftp.open(Path::new(remote))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn agent_helper() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/agent")
}

fn camera_args(output: &Path) -> Vec<String> {
    vec![
        "capture".to_string(),
        "usb-video".to_string(),
        "--output".to_string(),
        output.display().to_string(),
    ]
}

pub(super) fn capture_usb_video_frame(output: &Path) -> Result<()> {
    let status = Command::new(agent_helper())
        .args(camera_args(output))
        .status()?;
    if !status.success() {
        return Err("USB Video still capture failed".into());
    }
    Ok(())
}

fn check_interrupted(interrupted: &AtomicBool) -> Result<()> {
    if interrupted.load(Ordering::SeqCst) {
        Err("CRT qualification interrupted; restoring original configuration".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_attended_and_restore_commands() {
        assert_eq!(
            parse_args(&["qualify".into(), "--attended".into()]).unwrap(),
            QualifyAction::Attended { output: None }
        );
        assert_eq!(
            parse_args(&[
                "qualify".into(),
                "--attended".into(),
                "--out".into(),
                "/tmp/evidence".into(),
            ])
            .unwrap(),
            QualifyAction::Attended {
                output: Some(PathBuf::from("/tmp/evidence"))
            }
        );
        for pattern in ["full-ab-hold2", "full-ab-hold3", "full-ab-hold4"] {
            assert!(
                parse_args(&[
                    "probe".into(),
                    "--attended".into(),
                    "--pattern".into(),
                    pattern.into(),
                    "--seconds".into(),
                    "20".into(),
                    "--out".into(),
                    "/tmp/probe".into(),
                ])
                .is_ok()
            );
        }
        for pattern in [
            "motion-hold2",
            "motion-hold3",
            "motion-slow",
            "motion-color",
            "preloaded-ruler-slow",
            "preloaded-bars-slow",
        ] {
            let action = parse_args(&[
                "probe".into(),
                "--attended".into(),
                "--pattern".into(),
                pattern.into(),
                "--seconds".into(),
                "20".into(),
                "--out".into(),
                format!("/tmp/{pattern}"),
            ])
            .unwrap();
            assert_eq!(
                action,
                QualifyAction::Probe {
                    pattern: pattern.into(),
                    seconds: 20,
                    output: PathBuf::from(format!("/tmp/{pattern}")),
                }
            );
        }
        assert_eq!(
            parse_args(&["qualify".into(), "--restore".into()]).unwrap(),
            QualifyAction::Restore
        );
    }

    #[test]
    fn rejects_unattended_or_ambiguous_commands() {
        assert!(parse_args(&["qualify".into()]).is_err());
        assert!(
            parse_args(&[
                "qualify".into(),
                "--attended".into(),
                "--out".into(),
                "a".into(),
                "--out".into(),
                "b".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn parses_only_bounded_attended_probe_commands() {
        assert_eq!(
            parse_args(&[
                "probe".into(),
                "--attended".into(),
                "--pattern".into(),
                "fixed-a".into(),
                "--seconds".into(),
                "20".into(),
                "--out".into(),
                "/tmp/probe".into(),
            ])
            .unwrap(),
            QualifyAction::Probe {
                pattern: "fixed-a".into(),
                seconds: 20,
                output: PathBuf::from("/tmp/probe"),
            }
        );
        for invalid in ["unknown", "FIXED-A"] {
            assert!(
                parse_args(&[
                    "probe".into(),
                    "--attended".into(),
                    "--pattern".into(),
                    invalid.into(),
                    "--seconds".into(),
                    "20".into(),
                    "--out".into(),
                    "/tmp/probe".into(),
                ])
                .is_err()
            );
        }
        assert!(
            parse_args(&[
                "probe".into(),
                "--attended".into(),
                "--pattern".into(),
                "fixed-a".into(),
                "--seconds".into(),
                "21".into(),
                "--out".into(),
                "/tmp/probe".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn probe_command_is_self_restoring_and_does_not_change_display_mode() {
        let command = crt_probe_run_command("identical-flip", 20, "schema=1&output=crt-576p50");

        assert!(command.contains("trap cleanup EXIT HUP INT TERM"));
        assert!(command.contains("mister_magik_resume"));
        assert!(command.contains("MISTER_CRT_PROBE_PATTERN='identical-flip'"));
        assert!(command.contains(" ui crt_probe 20 "));
        assert!(!command.contains("display_apply"));
        assert!(!command.contains("reboot"));
    }

    #[test]
    fn probe_status_requires_zero_safety_failures() {
        let valid = "crt_probe_status_v1 schema=1 ok=1 pattern=fixed-a mode=crt-576p50 duration_ms=20000 slot_a_base=0x227e9000 slot_b_base=0x22fd2000 active_slot=1 writes=2 posts=2 flips=2 drops=0 final_pending=0 final_active_matches=1 unsafe_active_writes=0 pending_writes=0 cadence_misses=0 max_interval_us=20000 max_settle_us=19000 max_copy_us=1000 max_post_us=20000 last_sequence=3 reason=none";
        assert_eq!(parse_crt_probe_status(valid).unwrap(), valid);
        assert!(parse_crt_probe_status(&valid.replace("drops=0", "drops=1")).is_err());
        assert!(
            parse_crt_probe_status(
                &valid.replace("unsafe_active_writes=0", "unsafe_active_writes=1")
            )
            .is_err()
        );
        assert!(parse_crt_probe_status(&valid.replace("ok=1", "ok=0")).is_err());
    }

    #[test]
    fn standard_mode_matrix_has_expected_ini_mapping() {
        assert_eq!(CRT_MODES.len(), 4);
        assert_eq!(
            CRT_MODES
                .iter()
                .map(|mode| {
                    let (direct_video, menu_pal, forced_scandoubler) = mode.profile.settings();
                    (mode.output, direct_video, menu_pal, forced_scandoubler)
                })
                .collect::<Vec<_>>(),
            vec![
                ("crt-240p60", "1", "0", "0"),
                ("crt-288p50", "1", "1", "0"),
                ("crt-480p60", "1", "0", "1"),
                ("crt-576p50", "1", "1", "1"),
            ]
        );
    }

    #[test]
    fn requested_mode_must_match_mains_resolved_output() {
        let resolved = OriginalState {
            main: "MiSTer_MagiKDev".into(),
            output: "hdmi".into(),
        };
        let error = validate_resolved_mode(&resolved, CRT_MODES[0])
            .expect_err("a mismatched resolved mode must fail qualification")
            .to_string();
        assert!(error.contains("crt-240p60 resolved as hdmi"));
    }

    #[test]
    fn capture_is_fixed_to_usb_video_contract() {
        assert_eq!(
            camera_args(Path::new("/tmp/trial.jpg")),
            ["capture", "usb-video", "--output", "/tmp/trial.jpg"]
        );
    }

    #[test]
    fn trial_progress_requires_matching_mode_frames_and_flips() {
        let mode = CRT_MODES[0];
        assert!(validate_trial_progress(
            "crt_trial_status_v2 schema=2 ok=1 mode=crt-240p60 duration_ms=30000 frames=1800 flips=1800 reason=none",
            mode,
        )
        .is_ok());
        assert!(validate_trial_progress(
            "crt_trial_status_v2 schema=2 ok=1 mode=crt-240p60 duration_ms=30000 frames=1800 flips=0 reason=none",
            mode,
        )
        .is_err());
        assert!(validate_trial_progress(
            "crt_trial_status_v2 schema=2 ok=1 mode=crt-240p60 duration_ms=30000 frames=1800 flips=1799 reason=none",
            mode,
        )
        .is_err());
        assert!(validate_trial_progress(
            "crt_trial_status_v2 schema=2 ok=1 mode=crt-288p50 duration_ms=30000 frames=1500 flips=1500 reason=none",
            mode,
        )
        .is_err());
        assert!(validate_trial_progress(
            "crt_trial_status_v2 schema=2 ok=1 mode=crt-288p50 duration_ms=30000 frames=1500 flips=1500 reason=none",
            CRT_MODES[1],
        )
        .is_ok());
    }

    #[test]
    fn probe_parser_rejects_missing_contract_fields_and_uses_last_status() {
        let valid = "crt_probe_status_v1 schema=1 ok=1 pattern=fixed-a mode=crt-240p60 duration_ms=20000 slot_a_base=0x1 slot_b_base=0x2 writes=2 posts=2 flips=2 drops=0 final_pending=0 final_active_matches=1 unsafe_active_writes=0 pending_writes=0 reason=none";
        assert_eq!(
            parse_crt_probe_status(&format!("noise\nold\n{valid}\ntrailing")).unwrap(),
            valid
        );
        for required in ["writes=2", "posts=2", "flips=2", "reason=none"] {
            assert!(parse_crt_probe_status(&valid.replace(required, "removed=x")).is_err());
        }
        assert!(parse_crt_probe_status("untyped ok=1").is_err());
    }

    #[test]
    fn interrupt_and_output_directory_checks_fail_closed() {
        let interrupted = AtomicBool::new(false);
        check_interrupted(&interrupted).unwrap();
        interrupted.store(true, Ordering::SeqCst);
        assert!(check_interrupted(&interrupted).is_err());

        let output = std::env::temp_dir().join(format!("mister-crt-output-{}", std::process::id()));
        let _ = fs::remove_dir_all(&output);
        create_new_output_directory(&output).unwrap();
        assert!(create_new_output_directory(&output).is_err());
        fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn every_probe_pattern_has_an_attended_observation_contract() {
        for pattern in [
            "fixed-a",
            "fixed-b",
            "identical-flip",
            "slow-ab",
            "full-ab",
            "full-ab-hold2",
            "full-ab-hold3",
            "full-ab-hold4",
            "motion",
            "motion-hold2",
            "motion-hold3",
            "motion-slow",
            "motion-color",
            "preloaded-ruler-slow",
            "preloaded-bars-slow",
        ] {
            assert!(!probe_observation_prompt(pattern).is_empty());
        }
        assert_eq!(
            probe_observation_prompt("not-a-pattern"),
            "Observe the physical CRT."
        );
    }
}
