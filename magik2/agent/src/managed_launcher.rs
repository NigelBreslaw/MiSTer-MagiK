//! MagiK and Mini use Main's existing launcher environment and lifecycle.
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub const ENV_PATH: &str = "/media/fat/mister-magik-dev/launcher.env";
const BEGIN: &str = "# BEGIN magik2 managed launcher\n";
const END: &str = "# END magik2 managed launcher\n";
const LIMIT: usize = 64 * 1024;

pub fn require_dev(status: &Value) -> Result<(), String> {
    if status["executable_path"] != "/media/fat/MiSTer_MagiKDev" {
        return Err(
            "real MagiK deployment requires the running Dev Main; production is unchanged".into(),
        );
    }
    Ok(())
}

pub fn owns_ready_child(status: &Value, pid: u32) -> bool {
    status["launcher_state"] == "LauncherActive"
        && status["launcher_active"] == true
        && status["launcher_pid"].as_u64() == Some(u64::from(pid))
        && status["launcher_ready_phase"] == "ready"
        && status["fpga_owner"] == "magik"
        && matches!(status["input_proxy_protocol"].as_u64(), Some(2 | 3))
}

fn without_block(text: &str) -> Result<String, String> {
    match (text.find(BEGIN), text.find(END)) {
        (None, None) => Ok(text.to_owned()),
        (Some(start), Some(end))
            if end >= start
                && text.matches(BEGIN).count() == 1
                && text.matches(END).count() == 1 =>
        {
            Ok(format!("{}{}", &text[..start], &text[end + END.len()..]))
        }
        _ => Err("malformed magik2 launcher environment block".into()),
    }
}

fn quote(value: &str) -> Result<String, String> {
    if value.chars().any(char::is_control) {
        return Err("control character in launcher setting".into());
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

/// Change only our marked block. The existing file is never executed by this service.
pub fn update(path: &Path, values: Option<&[(&str, String)]>) -> Result<(), String> {
    let text = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => {
            if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                return Err("launcher environment is not a regular file".into());
            }
            let mut text = String::new();
            file.take((LIMIT + 1) as u64)
                .read_to_string(&mut text)
                .map_err(|e| e.to_string())?;
            if text.len() > LIMIT {
                return Err("launcher environment exceeds limit".into());
            }
            text
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.to_string()),
    };
    let mut next = without_block(&text)?;
    if let Some(values) = values {
        if !next.is_empty() && !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(BEGIN);
        for (key, value) in values {
            if !matches!(
                *key,
                "MISTER_MAGIK_PATH"
                    | "MISTER_MAGIK2_STATE_ROOT"
                    | "MISTER_MAGIK2_ARTIFACT_SHA256"
                    | "MISTER_MAGIK2_PROFILE_DIR"
                    | "SLINT_TEST_SERVER"
            ) {
                return Err("unsupported launcher environment key".into());
            }
            next.push_str(&format!("export {key}={}\n", quote(value)?));
        }
        next.push_str(END);
    }
    if next.len() > LIMIT {
        return Err("launcher environment exceeds limit".into());
    }
    if next == text {
        return Ok(());
    }
    let temporary = path.with_extension(format!("magik2-{}.next", std::process::id()));
    let result = (|| {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(next.as_bytes()).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

impl crate::Agent {
    pub(super) fn start_managed_application(
        &self,
        request: &crate::Envelope,
        test_server: Option<&str>,
        hash: &str,
        artifact: &str,
    ) -> crate::Envelope {
        let result = (|| -> Result<(), String> {
            require_dev(&crate::device::status()?)?;
            let env_path = Path::new(ENV_PATH);
            if env_path
                .parent()
                .unwrap()
                .canonicalize()
                .map_err(|e| e.to_string())?
                != env_path.parent().unwrap()
            {
                return Err(
                    "Dev launcher directory must not redirect to another installation".into(),
                );
            }
            fs::create_dir_all(&self.state_root).map_err(|e| e.to_string())?;
            self.stop_owned_process()?;
            crate::main_handoff("mister_magik_suspend\n")?;
            self.observation.clear_frame();
            for file in ["probe-ready.json", "measure-request"] {
                match fs::remove_file(self.state_root.join(file)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            let mut values = vec![
                (
                    "MISTER_MAGIK_PATH",
                    self.install_root
                        .join(artifact)
                        .to_string_lossy()
                        .into_owned(),
                ),
                (
                    "MISTER_MAGIK2_STATE_ROOT",
                    self.state_root.to_string_lossy().into_owned(),
                ),
                ("MISTER_MAGIK2_ARTIFACT_SHA256", hash.to_owned()),
            ];
            if let Some(endpoint) = test_server {
                values.push(("SLINT_TEST_SERVER", endpoint.to_owned()));
            }
            if let Some(profile) = request
                .fields
                .get("profile_id")
                .and_then(Value::as_str)
                .filter(|id| crate::is_plain_name(id))
            {
                values.push((
                    "MISTER_MAGIK2_PROFILE_DIR",
                    self.state_root
                        .join("profiles")
                        .join(profile)
                        .to_string_lossy()
                        .into_owned(),
                ));
            }
            update(env_path, Some(&values))?;
            crate::main_control::request("mister_magik_resume\n")?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                let status = crate::device::status()?;
                if let Some(pid) = status["launcher_pid"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    && owns_ready_child(&status, pid)
                    && self.ready_for(pid, hash)
                {
                    if crate::installed_hash(Path::new(&format!("/proc/{pid}/exe"))).as_deref()
                        != Some(hash)
                    {
                        return Err("Main launched a different artifact".into());
                    }
                    self.write_owned_process(pid, hash)?;
                    return Ok(());
                }
                if std::time::Instant::now() >= deadline {
                    return Err(format!("Main launcher readiness timed out: {status}"));
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        })();
        match result {
            Ok(()) => crate::response(
                &request.id,
                "started",
                serde_json::json!({"already_running":false,"ready":true,"main_managed":true}),
            ),
            Err(error) => crate::response(
                &request.id,
                "error",
                serde_json::json!({"code":"main-managed-start-failed","detail":error,"main_status":crate::device::status().ok()}),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_requires_main_ownership_and_physical_input_routing() {
        let good = serde_json::json!({"launcher_state":"LauncherActive", "launcher_active":true,
            "launcher_pid":42,"launcher_ready_phase":"ready","fpga_owner":"magik","input_proxy_protocol":2});
        assert!(owns_ready_child(&good, 42));
        assert!(!owns_ready_child(&good, 43));
        for (key, value) in [
            ("launcher_state", serde_json::json!("LauncherSuspended")),
            ("launcher_active", serde_json::json!(false)),
            ("fpga_owner", serde_json::json!("main")),
            ("launcher_ready_phase", serde_json::json!("waiting")),
            ("input_proxy_protocol", serde_json::json!(0)),
        ] {
            let mut bad = good.clone();
            bad[key] = value;
            assert!(!owns_ready_child(&bad, 42));
        }
        assert!(
            require_dev(&serde_json::json!({"executable_path":"/media/fat/MiSTer_MagiK"})).is_err()
        );
    }
    #[test]
    fn environment_preserves_operator_settings_and_removes_session_overrides() {
        let root = std::env::temp_dir().join(format!("magik2-managed-env-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("launcher.env");
        let original = "export EXISTING='keep me'\n";
        fs::write(&path, original).unwrap();
        update(
            &path,
            Some(&[
                ("MISTER_MAGIK_PATH", "/path/with ' quote".into()),
                ("SLINT_TEST_SERVER", "127.0.0.1:1234".into()),
            ]),
        )
        .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(original));
        assert!(content.contains("'\\''"));
        update(&path, Some(&[("MISTER_MAGIK_PATH", "/another".into())])).unwrap();
        assert!(
            !fs::read_to_string(&path)
                .unwrap()
                .contains("SLINT_TEST_SERVER")
        );
        update(&path, None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(root.join("missing"), &path).unwrap();
        assert!(update(&path, None).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
