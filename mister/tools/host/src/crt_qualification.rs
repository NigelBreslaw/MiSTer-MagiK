// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::{
    IniEdit, MenuOutputProfile, RebootMode, Result, acknowledged_main_command, connect,
    crt_trial_run_command, edit_remote_ini, exec, exec_checked, exec_checked_output, issue_reboot,
    parse_crt_trial_status, remote_write, wait_down, wait_launcher_ready, wait_up,
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
    Attended { output: Option<PathBuf> },
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
    }
}

fn parse_args(args: &[String]) -> Result<QualifyAction> {
    if args.first().map(String::as_str) != Some("qualify") {
        return Err("usage: mister crt qualify <--attended [--out DIRECTORY]|--restore>".into());
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
            "CRT trials completed but automatic restore failed: {restore}; run `mister crt qualify --restore`"
        )
        .into()),
        (Err(trial), Err(restore)) => Err(format!(
            "CRT qualification failed: {trial}; restore also failed: {restore}; run `mister crt qualify --restore`"
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
        "unfinished CRT qualification transaction exists; run `mister crt qualify --restore`".into()
    })
}

fn ensure_arming_clear(session: &Session) -> Result<()> {
    exec_checked(
        session,
        "CRT qualification arming preflight",
        "set -eu; for path in /media/fat/mister-magik/launcher.env /media/fat/mister-magik-dev/launcher.env /tmp/mister-magik/fs-fault-launcher.env /tmp/mister-magik/fs-fault-session /tmp/mister-magik/fs-fault.json /media/fat/mister-magik/rebuild-on-next-boot /media/fat/mister-magik-dev/rebuild-on-next-boot; do test ! -e \"$path\"; done",
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
}
