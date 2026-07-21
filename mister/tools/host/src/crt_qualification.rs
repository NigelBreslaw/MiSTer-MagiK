// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::{
    acknowledged_main_command, connect, crt_trial_run_command, edit_remote_ini, exec, exec_checked,
    exec_checked_output, issue_reboot, parse_crt_runtime_settings_reply, parse_crt_trial_status,
    remote_write, wait_down, wait_launcher_ready, wait_up, IniEdit, RebootMode, Result,
};
use serde_json::{json, Value};
use ssh2::Session;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const REMOTE_INI: &str = "/media/fat/MiSTer.ini";
const REMOTE_BACKUP: &str = "/media/fat/MiSTer.ini.mister-magik-crt-backup";
const REMOTE_JOURNAL: &str = "/media/fat/MiSTer.ini.mister-magik-crt-journal-v1";
const REMOTE_RESTORE_NEW: &str = "/media/fat/MiSTer.ini.mister-magik-crt-restore-new";
const CAMERA_DEVICE: &str = "USB Video";
const CAMERA_FORMAT: &str = "uyvy422";
const CAMERA_SIZE: &str = "1920x1080";
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
    direct_video: &'static str,
    menu_pal: &'static str,
    forced_scandoubler: &'static str,
    expected_rate: &'static str,
}

const CRT_MODES: [CrtMode; 4] = [
    CrtMode {
        output: "crt-240p60",
        direct_video: "1",
        menu_pal: "0",
        forced_scandoubler: "0",
        expected_rate: "15.734 kHz / 60.052 Hz",
    },
    CrtMode {
        output: "crt-288p50",
        direct_video: "1",
        menu_pal: "1",
        forced_scandoubler: "0",
        expected_rate: "15.734 kHz / 50.429 Hz",
    },
    CrtMode {
        output: "crt-480p60",
        direct_video: "1",
        menu_pal: "0",
        forced_scandoubler: "1",
        expected_rate: "31.469 kHz / 59.940 Hz",
    },
    CrtMode {
        output: "crt-576p50",
        direct_video: "1",
        menu_pal: "1",
        forced_scandoubler: "1",
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
    println!("Capture: {CAMERA_DEVICE}, {CAMERA_FORMAT}, {CAMERA_SIZE}");
    prompt_choice(
        interrupted,
        "Configure Morph for VGA RGBHV, 4:3, and neutral processing; type ready or abort: ",
        &["ready", "abort"],
    )
    .and_then(reject_abort)?;
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
        (Ok(all_passed), Ok(())) => {
            fs::write(
                output.join("summary.json"),
                serde_json::to_vec_pretty(&json!({
                    "schema": 1,
                    "kind": "mister-magik-morph-functional-qualification",
                    "all_modes_passed": all_passed,
                    "real_crt_qualified": false,
                }))?,
            )?;
            println!(
                "CRT Morph functional qualification complete: all_modes_passed={all_passed} real_crt_qualified=false"
            );
            if all_passed {
                Ok(())
            } else {
                Err("one or more CRT modes failed attended visual review".into())
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
    loop {
        check_interrupted(interrupted)?;
        let path = output.join("preflight.jpg");
        capture_frame(&path)?;
        println!("Preflight capture ready: {}", path.display());
        match prompt_choice(
            interrupted,
            "Inspect for stable lock and no speckling/noise; type clean, retry, or abort: ",
            &["clean", "retry", "abort"],
        )?
        .as_str()
        {
            "clean" => return Ok(()),
            "retry" => {}
            "abort" => return Err("CRT qualification aborted before device mutation".into()),
            _ => unreachable!(),
        }
    }
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
        fs::write(
            mode_directory.join("expected.json"),
            serde_json::to_vec_pretty(&json!({
                "output": mode.output,
                "direct_video": mode.direct_video,
                "menu_pal": mode.menu_pal,
                "forced_scandoubler": mode.forced_scandoubler,
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
    edit_remote_ini(
        &session,
        IniEdit::Crt {
            direct_video: mode.direct_video.to_string(),
            menu_pal: mode.menu_pal.to_string(),
            forced_scandoubler: mode.forced_scandoubler.to_string(),
        },
        false,
    )?;
    issue_reboot(&session, RebootMode::Supervised)?;
    drop(session);
    if !wait_down(40.0) || wait_up(120.0)? != 0 {
        return Err(format!("{} did not complete its bounded reboot", mode.output).into());
    }
    let session = connect(10)?;
    wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))?;
    let resolved = read_resolved_state(&session)?;
    if resolved.output != mode.output {
        return Err(format!(
            "Main resolved {} after requesting {}",
            resolved.output, mode.output
        )
        .into());
    }
    Ok(resolved)
}

fn capture_analyzer(directory: &Path, mode: CrtMode, interrupted: &AtomicBool) -> Result<()> {
    loop {
        prompt_choice(
            interrupted,
            &format!(
                "Open Morph RX Input Analyzer for {} and type ready or abort: ",
                mode.output
            ),
            &["ready", "abort"],
        )
        .and_then(reject_abort)?;
        let path = directory.join("analyzer.jpg");
        capture_frame(&path)?;
        println!("Analyzer capture ready: {}", path.display());
        match prompt_choice(
            interrupted,
            "Confirm stable lock and expected input rate; type accept, retry, or abort: ",
            &["accept", "retry", "abort"],
        )?
        .as_str()
        {
            "accept" => return Ok(()),
            "retry" => {}
            "abort" => return Err("CRT qualification aborted during analyzer review".into()),
            _ => unreachable!(),
        }
    }
}

fn run_mode_attempts(directory: &Path, mode: CrtMode, interrupted: &AtomicBool) -> Result<bool> {
    let mut attempt = 1_u32;
    loop {
        prompt_choice(
            interrupted,
            &format!(
                "Close the Morph OSD for {}; type run to capture the 30-second trial or abort: ",
                mode.output
            ),
            &["run", "abort"],
        )
        .and_then(reject_abort)?;
        let attempt_directory = directory.join(format!("attempt-{attempt:02}"));
        fs::create_dir(&attempt_directory)?;
        let video = attempt_directory.join("trial.mp4");
        let mut camera = spawn_video_capture(&video)?;
        let trial = run_crt_trial_once(mode);
        let camera_status = camera.wait()?;
        if !camera_status.success() {
            return Err(format!("USB Video capture failed for {}", mode.output).into());
        }
        let trial_status = trial?;
        fs::write(attempt_directory.join("trial-status.txt"), &trial_status)?;
        extract_review_frames(&video, &attempt_directory)?;
        println!("Review artifacts: {}", attempt_directory.display());
        let verdict = prompt_choice(
            interrupted,
            "After joint visual review, type pass, fail, retry, or abort: ",
            &["pass", "fail", "retry", "abort"],
        )?;
        fs::write(
            attempt_directory.join("verdict.json"),
            serde_json::to_vec_pretty(&json!({
                "mode": mode.output,
                "attempt": attempt,
                "verdict": verdict,
                "real_crt_qualified": false,
            }))?,
        )?;
        match verdict.as_str() {
            "pass" => return Ok(true),
            "fail" => return Ok(false),
            "retry" => attempt += 1,
            "abort" => return Err("CRT qualification aborted during visual review".into()),
            _ => unreachable!(),
        }
    }
}

fn run_crt_trial_once(mode: CrtMode) -> Result<String> {
    let session = connect(10)?;
    let output = exec_checked_output(
        &session,
        "resolved CRT mode",
        &acknowledged_main_command("mister_magik_settings_get_v1"),
    )?;
    let runtime_settings = parse_crt_runtime_settings_reply(&output.stdout)?;
    exec_checked(
        &session,
        "scene suspend",
        &acknowledged_main_command("mister_magik_suspend"),
    )?;
    exec_checked(
        &session,
        "operator CRT trial",
        &crt_trial_run_command(&runtime_settings),
    )?;
    let output = exec_checked_output(
        &session,
        "CRT trial status",
        "sed -n '/^crt_trial_status_v2 /p' /tmp/mister-magik-crt_trial.log | tail -n 1",
    )?;
    let status = parse_crt_trial_status(&output.stdout)?;
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
    for name in ["frames", "flips"] {
        let value = field(name)
            .ok_or_else(|| format!("CRT trial status omitted {name}"))?
            .parse::<u64>()?;
        if value == 0 {
            return Err(format!("CRT trial status reported no advancing {name}").into());
        }
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
    if let Some(expected) = expected {
        if expected.main != journal_expected.main || expected.output != journal_expected.output {
            return Err("CRT recovery journal does not match the active session".into());
        }
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
    let reply = exec_checked_output(
        session,
        "resolved video mode",
        &acknowledged_main_command("mister_magik_settings_get_v1"),
    )?;
    let output = parse_resolved_output(&reply.stdout)?.to_string();
    Ok(OriginalState { main, output })
}

fn parse_resolved_output(reply: &str) -> Result<&str> {
    let settings = reply
        .trim()
        .strip_prefix("ok SettingsV1 ")
        .ok_or("Main did not return runtime settings v1")?;
    settings
        .split_ascii_whitespace()
        .find_map(|field| field.strip_prefix("output="))
        .ok_or_else(|| "Main runtime settings omitted output".into())
}

fn remote_read_bytes(session: &Session, remote: &str) -> Result<Vec<u8>> {
    let sftp = session.sftp()?;
    let mut file = sftp.open(Path::new(remote))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn camera_helper() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/host-camera")
}

fn camera_args(command: &str, output: &Path) -> Vec<String> {
    let mut args = vec![
        command.to_string(),
        "--device".to_string(),
        CAMERA_DEVICE.to_string(),
        "--size".to_string(),
        CAMERA_SIZE.to_string(),
        "--pixel-format".to_string(),
        CAMERA_FORMAT.to_string(),
        "--framerate".to_string(),
        "30".to_string(),
    ];
    if command == "video" {
        args.extend(["--duration".to_string(), "32".to_string()]);
    }
    args.extend(["--output".to_string(), output.display().to_string()]);
    args
}

fn capture_frame(output: &Path) -> Result<()> {
    let status = Command::new(camera_helper())
        .args(camera_args("frame", output))
        .status()?;
    if !status.success() {
        return Err("USB Video still capture failed".into());
    }
    Ok(())
}

fn spawn_video_capture(output: &Path) -> Result<std::process::Child> {
    Ok(Command::new(camera_helper())
        .args(camera_args("video", output))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?)
}

fn extract_review_frames(video: &Path, directory: &Path) -> Result<()> {
    for (label, seconds) in [("start", "1"), ("middle", "16"), ("end", "30")] {
        let output = directory.join(format!("frame-{label}.jpg"));
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-ss",
                seconds,
                "-i",
            ])
            .arg(video)
            .args(["-frames:v", "1", "-update", "1"])
            .arg(output)
            .status()?;
        if !status.success() {
            return Err(format!("failed to extract {label} CRT review frame").into());
        }
    }
    Ok(())
}

fn prompt_choice(interrupted: &AtomicBool, prompt: &str, choices: &[&str]) -> Result<String> {
    loop {
        check_interrupted(interrupted)?;
        print!("{prompt}");
        io::stdout().flush()?;
        let mut answer = String::new();
        match io::stdin().read_line(&mut answer) {
            Ok(0) => return Err("interactive input closed".into()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                check_interrupted(interrupted)?;
                continue;
            }
            Err(error) => return Err(error.into()),
        }
        check_interrupted(interrupted)?;
        let answer = answer.trim().to_ascii_lowercase();
        if choices.contains(&answer.as_str()) {
            return Ok(answer);
        }
        println!("Expected one of: {}", choices.join(", "));
    }
}

fn reject_abort(answer: String) -> Result<()> {
    if answer == "abort" {
        Err("CRT qualification aborted by operator".into())
    } else {
        Ok(())
    }
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
        assert!(parse_args(&[
            "qualify".into(),
            "--attended".into(),
            "--out".into(),
            "a".into(),
            "--out".into(),
            "b".into(),
        ])
        .is_err());
    }

    #[test]
    fn standard_mode_matrix_has_expected_ini_mapping() {
        assert_eq!(CRT_MODES.len(), 4);
        assert_eq!(
            CRT_MODES
                .iter()
                .map(|mode| (
                    mode.output,
                    mode.direct_video,
                    mode.menu_pal,
                    mode.forced_scandoubler,
                ))
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
    fn capture_is_fixed_to_usb_video_contract() {
        let args = camera_args("video", Path::new("/tmp/trial.mp4"));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--device", "USB Video"]));
        assert!(args.windows(2).any(|pair| pair == ["--size", "1920x1080"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--pixel-format", "uyvy422"]));
        assert!(args.windows(2).any(|pair| pair == ["--duration", "32"]));
    }

    #[test]
    fn parses_resolved_main_output() {
        assert_eq!(
            parse_resolved_output("ok SettingsV1 schema=1 output=crt-576p50 other=1\n").unwrap(),
            "crt-576p50"
        );
        assert!(parse_resolved_output("ok SettingsV1 schema=1").is_err());
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
            "crt_trial_status_v2 schema=2 ok=1 mode=crt-288p50 duration_ms=30000 frames=1500 flips=1500 reason=none",
            mode,
        )
        .is_err());
    }
}
