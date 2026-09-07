//! Typed retained device controls. No shell, display matrix, or automatic reboot.
use crate::{Agent, Envelope, main_control, response};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

const MAIN_STATUS: &str = "/tmp/mister-magik/main-status.json";
pub const OPERATIONS: &[&str] = &[
    "device-status",
    "mode-status",
    "mode-set",
    "device-reboot",
    "device-recover",
    "media-operation",
    "device-evidence",
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
    for root in [
        "/media/fat/mister-magik-dev/crashes",
        "/media/fat/mister-magik/crashes",
    ] {
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
                    crashes.push(evidence_file(path));
                }
            }
            Err(error) => crashes.push(json!({"path":root,"error":error.to_string()})),
        }
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
                    "display-set"
                        | "media-operation"
                        | "mode-set"
                        | "device-reboot"
                        | "device-recover"
                ) && !request.fields.is_empty())
            {
                return Err("unexpected device operation arguments".into());
            }
            match request.op.as_str() {
                "device-status" => status(),
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
}
