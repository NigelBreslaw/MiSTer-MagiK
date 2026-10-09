//! Typed retained device controls. No shell, display matrix, or automatic reboot.
use crate::{Agent, Envelope, main_control, response};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

const MAIN_STATUS: &str = "/tmp/mister-magik/main-status.json";
pub const OPERATIONS: &[&str] = &[
    "device-status",
    "application-install-inspect",
    "application-install-recover",
    "crash-report-read",
    "crash-report-delete",
    "input-probe",
    "mode-status",
    "mode-set",
    "device-reboot",
    "device-recover",
    "media-operation",
    "device-evidence",
    "fpga-evidence",
    "launcher-restart",
    "launcher-return",
    "display-status",
    "display-set",
];
const DISPLAY_MODES: &[&str] = &[
    "auto",
    "hdmi-1280x720p60",
    "hdmi-1366x768p60",
    "hdmi-1920x1080p60",
    "hdmi-1920x1200p60",
    "hdmi-2048x1536p60",
    "hdmi-2560x1440p60",
    "crt-240p60",
    "crt-288p50",
    "crt-480p60",
    "crt-576p50",
];

fn read(path: &Path, limit: usize) -> Result<String, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!("{} exceeds {limit} bytes", path.display()));
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

pub fn status() -> Result<Value, String> {
    serde_json::from_str(&read(Path::new(MAIN_STATUS), 16384)?).map_err(|e| e.to_string())
}

fn evidence_file(path: &Path) -> Value {
    match read(path, 8192) {
        Ok(text) => json!({"path":path,"text":text}),
        Err(error) => json!({"path":path,"error":error}),
    }
}

const CRASH_REPORT_LIMIT: u64 = 65536;
const CRASH_ROOTS: [&str; 2] = [
    "/media/fat/mister-magik-dev/crashes",
    "/media/fat/mister-magik/crashes",
];

fn crash_report_path(path: &str, roots: &[&Path]) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if !roots.iter().any(|root| path.parent() == Some(*root))
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || !path.file_name().is_some_and(|name| {
            name == "latest.json" || name.to_string_lossy().starts_with("report-")
        })
        || path.extension().is_none_or(|extension| extension != "json")
        || !fs::symlink_metadata(&path)
            .map_err(|e| e.to_string())?
            .is_file()
        || path.canonicalize().map_err(|e| e.to_string())? != path
    {
        return Err("expected a regular report JSON directly inside a crash directory".into());
    }
    Ok(path)
}

fn restore_claim_without_replacing(claimed: &Path, original: &Path) -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(claimed.as_os_str().as_bytes())
            .map_err(|error| error.to_string())?;
        let destination = std::ffi::CString::new(original.as_os_str().as_bytes())
            .map_err(|error| error.to_string())?;
        // SAFETY: both pathnames are live NUL-terminated strings. No-replace
        // keeps a newer report published at the original path untouched.
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        fs::hard_link(claimed, original).map_err(|error| error.to_string())?;
        fs::remove_file(claimed).map_err(|error| error.to_string())
    }
}

fn delete_crash_report(path: &Path, expected: &str) -> Result<Value, String> {
    delete_crash_report_with(path, expected, || Ok(()))
}

fn delete_crash_report_with(
    path: &Path,
    expected: &str,
    after_claim: impl FnOnce() -> Result<(), String>,
) -> Result<Value, String> {
    // This alias is replaced independently by crash writers. Only immutable,
    // individually named reports can be deleted after checksum verification.
    if path.file_name().is_some_and(|name| name == "latest.json") {
        return Err(
            "latest.json may be replaced by a crash writer; delete its named report instead".into(),
        );
    }
    let parent = path.parent().ok_or("crash directory missing")?;
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let claimed = parent.join(format!(
        "report-delete-{unique}-{}.json",
        std::process::id()
    ));
    File::options()
        .write(true)
        .create_new(true)
        .open(&claimed)
        .map_err(|error| error.to_string())?;
    if let Err(error) = fs::rename(path, &claimed) {
        let _ = fs::remove_file(&claimed);
        return Err(error.to_string());
    }
    // The owned claim is a readable named report even after service failure.
    // Writers may now replace the original path; we only unlink this claim.
    let result = (|| {
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        after_claim()?;
        use sha2::{Digest, Sha256};
        use std::os::unix::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&claimed)
            .map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() > 1024 * 1024 {
            return Err("crash report exceeds deletion limit".into());
        }
        let actual: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if actual != expected {
            return Err("crash report changed; deletion refused".into());
        }
        let report: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if report["schema"] != "mister-magik-crash-report-v1" {
            return Err("unsupported crash report; deletion refused".into());
        }
        fs::remove_file(&claimed).map_err(|e| e.to_string())?;
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(
            json!({"deleted":true,"path":path,"sha256":expected,"report_id":report["report_id"],"replacement_preserved":path.exists()}),
        )
    })();
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            if !claimed.exists() {
                return Err(error);
            }
            let restored = restore_claim_without_replacing(&claimed, path);
            let _ = File::open(parent).and_then(|file| file.sync_all());
            match restored {
                Ok(()) => Err(error),
                Err(restore) => Err(format!(
                    "{error}; report restoration failed: {restore}; retained report: {}",
                    claimed.display()
                )),
            }
        }
    }
}

fn allow_crash_deletion(path: &Path, main: Option<&Value>) -> Result<(), String> {
    if path.parent() != Some(Path::new(CRASH_ROOTS[0])) {
        return Err("Dev service cannot delete production crash reports".into());
    }
    if main.and_then(reported_crash_path).as_ref() == Some(&path.to_path_buf()) {
        return Err("Main still identifies this as its current crash; deletion refused".into());
    }
    Ok(())
}

/// The report the main status names as the last crash, if it lies inside a crash
/// directory. Nothing outside those directories is ever read.
fn reported_crash_path(status: &Value) -> Option<PathBuf> {
    let path = PathBuf::from(status["last_crash_report"].as_str()?);
    let inside = CRASH_ROOTS.iter().any(|root| path.starts_with(root));
    (inside && !path.components().any(|part| part == Component::ParentDir)).then_some(path)
}

/// A crash report, bounded. Whole when it fits, otherwise its first and last
/// halves, which hold the cause and the backtrace; the middle is skipped.
fn crash_file(path: &Path) -> Value {
    let read_part = |offset: u64, length: u64| -> Result<String, String> {
        use std::io::{Seek, SeekFrom};
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(length)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    let size = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) => return json!({"path":path,"error":error.to_string()}),
    };
    let half = CRASH_REPORT_LIMIT / 2;
    let parts = if size <= CRASH_REPORT_LIMIT {
        read_part(0, size).map(|text| json!({"text":text}))
    } else {
        read_part(0, half).and_then(|head| {
            read_part(size - half, half)
                .map(|tail| json!({"truncated":true,"bytes":size,"head":head,"tail":tail}))
        })
    };
    match parts {
        Ok(Value::Object(mut fields)) => {
            fields.insert("path".into(), json!(path));
            Value::Object(fields)
        }
        Ok(other) => other,
        Err(error) => json!({"path":path,"error":error}),
    }
}

fn evidence() -> Value {
    let mut crashes = Vec::new();
    let files: Vec<_> = [
        "/tmp/mister-magik-boot-analytics.tsv",
        "/tmp/mister-magik/events.jsonl",
        "/tmp/mister-magik-slint.log",
        "/tmp/mister-magik/latch-failure.json",
    ]
    .iter()
    .map(|path| json!({"path":path,"tail":crate::log_tail(Path::new(path))}))
    .collect();
    for root in CRASH_ROOTS {
        match fs::read_dir(root) {
            Ok(entries) => {
                let mut paths: Vec<_> = entries
                    .take(512)
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                    .collect();
                paths.sort();
                if let Some(path) = paths.last() {
                    crashes.push(crash_file(path));
                }
            }
            Err(error) => crashes.push(json!({"path":root,"error":error.to_string()})),
        }
    }
    // The newest report by name is not always the one the launcher reports.
    if let Some(path) = status().ok().as_ref().and_then(reported_crash_path)
        && !crashes.iter().any(|crash| crash["path"] == json!(path))
    {
        crashes.push(crash_file(&path));
    }
    json!({"main_status":status().map_err(|e|json!({"error":e})).unwrap_or_else(|e|e),
        "main_log": {"path":"/tmp/mister-magik-main.log","tail":crate::log_tail(Path::new("/tmp/mister-magik-main.log"))},
        "crashes":crashes,"crash_scan_limit":512,"files":files,
        "boot_id":evidence_file(Path::new("/proc/sys/kernel/random/boot_id")),
        "uptime":evidence_file(Path::new("/proc/uptime"))})
}

fn wait_launcher(previous: Option<u64>) -> Result<Value, String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let state = status()?;
        let pid = state["launcher_pid"].as_u64().filter(|pid| *pid > 0);
        if state["launcher_active"] == true
            && state["launcher_ready_phase"] == "ready"
            && pid.is_some()
            && (previous.is_none() || pid != previous)
        {
            return Ok(state);
        }
        if Instant::now() >= deadline {
            return Err(
                "Main acknowledged, but launcher did not become ready within 15 seconds".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn display_command(fields: &serde_json::Map<String, Value>) -> Result<String, String> {
    let mode = fields
        .get("mode")
        .and_then(Value::as_str)
        .ok_or("display mode is required")?;
    if fields.len() != 2 + usize::from(fields.contains_key("acknowledge_31khz"))
        || (matches!(mode, "crt-480p60" | "crt-576p50")
            && fields.get("acknowledge_31khz") != Some(&Value::Bool(true)))
        || fields.get("attended") != Some(&Value::Bool(true))
        || !DISPLAY_MODES.contains(&mode)
    {
        return Err("display-set requires a supported mode and explicit attendance".into());
    }
    Ok(format!(
        "mister_magik_display_apply_headless_v1 mode={mode}\n"
    ))
}

fn display_field<'a>(reply: &'a str, name: &str) -> Result<&'a str, String> {
    reply
        .split_whitespace()
        .find_map(|field| field.strip_prefix(name))
        .ok_or_else(|| format!("Main display reply omitted {name}"))
}

fn set_display(fields: &serde_json::Map<String, Value>) -> Result<Value, String> {
    let command = display_command(fields)?;
    let original = main_control::request("mister_magik_display_get_v1\n")?;
    if display_field(&original, "pending=")? != "none" {
        return Err("a display transaction is already pending".into());
    }
    let original_mode = display_field(&original, "active=")?.to_owned();
    if original_mode == fields["mode"].as_str().unwrap_or("") {
        return Ok(json!({"main_status":status()?,"reply":original,"unchanged":true}));
    }
    let previous = status()?["launcher_pid"].as_u64();
    let result = (|| {
        main_control::request(&command)?;
        let state = wait_launcher(previous)?;
        main_control::request("mister_magik_display_confirm_v1\n")?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let reply = main_control::request("mister_magik_display_get_v1\n")?;
            if display_field(&reply, "phase=")? == "failed" {
                return Err("display persistence failed".into());
            }
            if display_field(&reply, "pending=")? == "none" {
                if fields["mode"] != "auto"
                    && display_field(&reply, "active=")? != fields["mode"].as_str().unwrap_or("")
                {
                    return Err("Main committed a different display mode".into());
                }
                return Ok(json!({"main_status":state,"reply":reply}));
            }
            if Instant::now() >= deadline {
                return Err("display persistence timed out".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })();
    result.map_err(|error: String| {
        let restore = (|| -> Result<(), String> {
            main_control::request("mister_magik_display_cancel_v1\n")?;
            wait_launcher(None)?;
            let reply = main_control::request("mister_magik_display_get_v1\n")?;
            if display_field(&reply, "active=")? != original_mode
                || display_field(&reply, "pending=")? != "none"
            {
                return Err(format!("original display has not been restored: {reply}"));
            }
            Ok(())
        })();
        format!("{error}; display restoration: {restore:?}")
    })
}

impl Agent {
    pub(super) fn device_operation(&self, request: &Envelope, body: &[u8]) -> Envelope {
        let result = (|| -> Result<Value, String> {
            if !body.is_empty()
                || (!matches!(
                    request.op.as_str(),
                    "input-probe"
                        | "display-set"
                        | "media-operation"
                        | "mode-set"
                        | "device-reboot"
                        | "device-recover"
                        | "crash-report-read"
                        | "crash-report-delete"
                ) && !request.fields.is_empty())
            {
                return Err("unexpected device operation arguments".into());
            }
            match request.op.as_str() {
                "device-status" => status(),
                "application-install-inspect" => self.app_install_inspect(),
                "application-install-recover" => {
                    self.recover_app_install()?;
                    self.app_install_inspect()
                }
                "crash-report-read" | "crash-report-delete" => {
                    let deleting = request.op == "crash-report-delete";
                    if request.fields.len() != if deleting { 2 } else { 1 } {
                        return Err(
                            "crash report operation requires path and, for deletion, sha256".into(),
                        );
                    }
                    let path = crash_report_path(
                        request
                            .fields
                            .get("path")
                            .and_then(Value::as_str)
                            .ok_or("report path required")?,
                        &CRASH_ROOTS.map(Path::new),
                    )?;
                    if deleting {
                        allow_crash_deletion(&path, status().ok().as_ref())?;
                        delete_crash_report(
                            &path,
                            request
                                .fields
                                .get("sha256")
                                .and_then(Value::as_str)
                                .ok_or("report sha256 required")?,
                        )
                    } else {
                        Ok(json!({"report":crash_file(&path),"sha256":crate::media::hash(&path)?}))
                    }
                }
                "input-probe" => crate::input_probe::run(&request.fields),
                "mode-status" => crate::mode::status(),
                "mode-set" => crate::mode::set(&request.fields),
                "device-reboot" => crate::mode::reboot(&request.fields),
                "device-recover" => {
                    if request.fields.len() != 1
                        || request.fields.get("attended") != Some(&Value::Bool(true))
                    {
                        return Err("recovery requires explicit attendance".into());
                    }
                    crate::mode::disarm()?;
                    self.stop_owned_process()?;
                    main_control::handoff("mister_magik_resume\n")?;
                    wait_launcher(None)
                }
                "media-operation" => crate::media::run(&request.fields),
                "device-evidence" => Ok(evidence()),
                "fpga-evidence" => crate::fpga_evidence::capture(),
                "display-status" => {
                    Ok(json!({"reply":main_control::request("mister_magik_display_get_v1\n")?}))
                }
                "launcher-restart" | "launcher-return" => {
                    let previous = status()?["launcher_pid"].as_u64();
                    self.stop_owned_process()?;
                    let command = if request.op == "launcher-restart" {
                        "mister_magik_restart_launcher\n"
                    } else {
                        "mister_magik_return_to_launcher\n"
                    };
                    main_control::request(command)?;
                    wait_launcher(if request.op == "launcher-restart" {
                        previous
                    } else {
                        None
                    })
                }
                "display-set" => set_display(&request.fields),
                _ => Err("unsupported device operation".into()),
            }
        })();
        match result {
            Ok(value) => response(&request.id, "device-result", value),
            Err(error) => response(
                &request.id,
                "error",
                json!({"code":"device-operation-failed","detail":error}),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deletion_is_dev_only_and_does_not_depend_on_readable_main_status() {
        let dev = Path::new("/media/fat/mister-magik-dev/crashes/report-reviewed.json");
        let production = Path::new("/media/fat/mister-magik/crashes/report-reviewed.json");
        assert!(allow_crash_deletion(dev, None).is_ok());
        assert!(allow_crash_deletion(production, None).is_err());
        let current = json!({"last_crash_report":dev});
        assert!(allow_crash_deletion(dev, Some(&current)).is_err());
        assert!(allow_crash_deletion(dev, Some(&json!({"last_crash_report":""}))).is_ok());
    }
    #[test]
    fn display_changes_are_closed_and_attended() {
        let fields = json!({"mode":"crt-240p60","attended":true});
        assert!(display_command(fields.as_object().unwrap()).is_ok());
        for fields in [
            json!({"mode":"crt-240p60"}),
            json!({"mode":"crt-480p60","attended":true}),
            json!({"mode":"x;reboot","attended":true}),
            json!({"mode":"crt-240p60","attended":true,"extra":1}),
        ] {
            assert!(display_command(fields.as_object().unwrap()).is_err());
        }
    }
    #[test]
    fn evidence_reports_oversize_and_missing_files() {
        let root = std::env::temp_dir().join(format!("magik-evidence-{}", std::process::id()));
        fs::write(&root, vec![b'x'; 8193]).unwrap();
        assert!(evidence_file(&root).get("error").is_some());
        fs::remove_file(&root).unwrap();
        assert!(evidence_file(&root).get("error").is_some());
    }
    #[test]
    fn only_a_report_inside_a_crash_directory_is_read() {
        let named = |path: &str| reported_crash_path(&json!({"last_crash_report":path}));
        let inside = "/media/fat/mister-magik-dev/crashes/report-main-1-2.json";
        assert_eq!(named(inside), Some(PathBuf::from(inside)));
        assert!(named("/media/fat/mister-magik/crashes/report-slint-3.json").is_some());
        assert!(named("/etc/passwd").is_none());
        assert!(named("/media/fat/mister-magik-dev/crashes/../../secret.json").is_none());
        assert!(named("/media/fat/mister-magik-dev/crashes-other/x.json").is_none());
        assert!(reported_crash_path(&json!({})).is_none());
    }
    #[test]
    fn latest_report_alias_cannot_be_deleted_even_with_a_matching_digest() {
        let root = std::env::temp_dir().join(format!("magik-latest-report-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("latest.json");
        let body = br#"{"schema":"mister-magik-crash-report-v1","report_id":"latest"}"#;
        fs::write(&path, body).unwrap();
        let hash = crate::media::hash(&path).unwrap();
        assert!(
            delete_crash_report(&path, &hash)
                .unwrap_err()
                .contains("latest.json")
        );
        assert_eq!(fs::read(&path).unwrap(), body);
        let replacement =
            br#"{"schema":"mister-magik-crash-report-v1","report_id":"new-unreviewed"}"#;
        fs::write(&path, replacement).unwrap();
        assert!(delete_crash_report(&path, &hash).is_err());
        assert_eq!(fs::read(&path).unwrap(), replacement);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_report_replacement_is_never_deleted_or_overwritten() {
        for matching in [true, false] {
            let root = std::env::temp_dir().join(format!(
                "magik-report-replace-{matching}-{}",
                std::process::id()
            ));
            fs::create_dir_all(&root).unwrap();
            let path = root.join("report-reviewed.json");
            let old = br#"{"schema":"mister-magik-crash-report-v1","report_id":"reviewed"}"#;
            let new = br#"{"schema":"mister-magik-crash-report-v1","report_id":"unreviewed"}"#;
            fs::write(&path, old).unwrap();
            let hash = if matching {
                crate::media::hash(&path).unwrap()
            } else {
                "0".repeat(64)
            };
            let result = delete_crash_report_with(&path, &hash, || {
                fs::write(&path, new).map_err(|error| error.to_string())
            });
            assert_eq!(fs::read(&path).unwrap(), new);
            if matching {
                assert_eq!(result.unwrap()["replacement_preserved"], true);
            } else {
                let error = result.unwrap_err();
                let retained = error.split("retained report: ").nth(1).unwrap();
                assert_eq!(fs::read(retained).unwrap(), old);
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn crash_deletion_requires_a_report_in_the_allowed_directory_and_matching_hash() {
        let directory =
            std::env::temp_dir().join(format!("magik-crash-delete-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let path = directory.join("report-fixed.json");
        fs::write(
            &path,
            br#"{"schema":"mister-magik-crash-report-v1","report_id":"fixed"}"#,
        )
        .unwrap();
        let roots = [directory.as_path()];
        let validated = crash_report_path(path.to_str().unwrap(), &roots).unwrap();
        assert!(delete_crash_report(&validated, &"0".repeat(64)).is_err());
        assert!(path.exists());
        assert!(
            crash_report_path(
                directory.join("../report-fixed.json").to_str().unwrap(),
                &roots
            )
            .is_err()
        );
        let symlink = directory.join("report-link.json");
        std::os::unix::fs::symlink(&path, &symlink).unwrap();
        assert!(crash_report_path(symlink.to_str().unwrap(), &roots).is_err());
        let hash = crate::media::hash(&path).unwrap();
        assert_eq!(
            delete_crash_report(&validated, &hash).unwrap()["deleted"],
            true
        );
        assert!(!path.exists());
        fs::remove_file(symlink).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn crash_reports_keep_their_head_and_tail_when_large() {
        let path = std::env::temp_dir().join(format!("magik-crash-{}", std::process::id()));
        let half = (CRASH_REPORT_LIMIT / 2) as usize;
        let mut big = vec![b'h'; half];
        big.extend(vec![b'm'; 5000]);
        big.extend(vec![b't'; half]);
        fs::write(&path, &big).unwrap();
        let report = crash_file(&path);
        assert_eq!(report["truncated"], true);
        assert_eq!(report["bytes"], big.len());
        assert!(report["head"].as_str().unwrap().bytes().all(|b| b == b'h'));
        assert!(report["tail"].as_str().unwrap().bytes().all(|b| b == b't'));
        fs::write(&path, b"{\"signal\":6}").unwrap();
        let small = crash_file(&path);
        assert_eq!(small["text"], "{\"signal\":6}");
        assert!(small.get("truncated").is_none());
        fs::remove_file(&path).unwrap();
        assert!(crash_file(&path).get("error").is_some());
    }
}
