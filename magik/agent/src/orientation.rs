//! Headless Dev monitor orientation, using Main's owned launcher lifecycle.
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const SETTINGS: &str = "/media/fat/mister-magik-dev/settings.json";
const LIMIT: usize = 64 * 1024;
const ORIENTATIONS: [&str; 3] = ["normal", "monitor-clockwise", "monitor-counterclockwise"];

fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("orientation settings must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take((LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > LIMIT {
        return Err("orientation settings exceed 64 KiB".into());
    }
    Ok(Some(bytes))
}

fn document(bytes: Option<&[u8]>) -> Result<Value, String> {
    let value: Value = match bytes {
        Some(bytes) => serde_json::from_slice(bytes).map_err(|e| e.to_string())?,
        None => json!({"version": 1}),
    };
    if !value.is_object() || value["version"] != 1 {
        return Err("orientation requires version 1 settings".into());
    }
    if let Some(orientation) = value.get("screen_orientation")
        && !ORIENTATIONS.contains(&orientation.as_str().unwrap_or(""))
    {
        return Err("saved orientation is invalid".into());
    }
    Ok(value)
}

fn write(path: &Path, bytes: Option<&[u8]>) -> Result<(), String> {
    let parent = path.parent().ok_or("settings parent missing")?;
    if let Some(bytes) = bytes {
        let temporary = path.with_extension(format!("orientation-{}.next", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        let result = (|| {
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|e| e.to_string())?;
            fs::rename(&temporary, path).map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
    } else if let Err(error) = fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error.to_string());
    }
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())
}

fn change(
    path: &Path,
    orientation: &str,
    mut lifecycle: impl FnMut(bool) -> Result<Value, String>,
) -> Result<Value, String> {
    // Suspend before reading so no launcher writer can race this edit.
    if let Err(error) = lifecycle(false) {
        return Err(format!("{error}; Main resumption: {:?}", lifecycle(true)));
    }
    let prepared = (|| {
        let original = read(path)?;
        let mut value = document(original.as_deref())?;
        value["screen_orientation"] = orientation.into();
        let next = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
        if next.len() > LIMIT {
            return Err("updated orientation settings exceed 64 KiB".into());
        }
        Ok::<_, String>((original, next))
    })();
    let (original, next) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return Err(format!("{error}; Main resumption: {:?}", lifecycle(true))),
    };
    let result = write(path, Some(&next)).and_then(|()| lifecycle(true));
    match result {
        Ok(state) => Ok(json!({"configured_orientation": orientation, "main_status": state})),
        Err(error) => {
            // A failed acknowledgement may have resumed the launcher. Suspend
            // it again before restoring settings, then resume exactly once.
            let restore = lifecycle(false).and_then(|_| write(path, original.as_deref()));
            let resumed = lifecycle(true);
            Err(format!(
                "{error}; orientation restoration: {restore:?}; Main resumption: {resumed:?}"
            ))
        }
    }
}

pub(super) fn status() -> Result<Value, String> {
    let state = crate::device::status()?;
    crate::managed_launcher::require_dev(&state)?;
    let value = document(read(Path::new(SETTINGS))?.as_deref())?;
    Ok(
        json!({"configured_orientation": value.get("screen_orientation").and_then(Value::as_str).unwrap_or("normal"), "main_status": state}),
    )
}

pub(super) fn set(fields: &serde_json::Map<String, Value>) -> Result<Value, String> {
    let orientation = fields
        .get("orientation")
        .and_then(Value::as_str)
        .unwrap_or("");
    if fields.len() != 2
        || fields.get("attended") != Some(&Value::Bool(true))
        || !ORIENTATIONS.contains(&orientation)
    {
        return Err(
            "orientation-set requires a supported orientation and explicit attendance".into(),
        );
    }
    let state = crate::device::status()?;
    crate::managed_launcher::require_dev(&state)?;
    if state["launcher_state"] != "LauncherActive"
        || state["launcher_ready_phase"] != "ready"
        || state["deploy_locked"] == true
    {
        return Err(
            "orientation-set requires an active ready Dev launcher without a deployment".into(),
        );
    }
    change(Path::new(SETTINGS), orientation, |resume| {
        if resume {
            crate::main_control::request("mister_magik_resume\n")?;
            crate::device::wait_launcher(None)
        } else {
            crate::main_control::handoff("mister_magik_suspend\n")?;
            Ok(Value::Null)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_or_unattended_requests_are_rejected_before_device_access() {
        for fields in [
            json!({"orientation":"normal"}),
            json!({"orientation":"normal","attended":false}),
            json!({"orientation":"sideways","attended":true}),
            json!({"orientation":"normal","attended":true,"path":"/tmp/settings"}),
        ] {
            assert!(
                set(fields.as_object().unwrap())
                    .unwrap_err()
                    .contains("requires a supported orientation")
            );
        }
    }

    #[test]
    fn existing_temporary_file_is_never_deleted_or_overwritten() {
        let path = fixture("temporary");
        let temporary = path.with_extension(format!("orientation-{}.next", std::process::id()));
        fs::write(&temporary, b"unrelated").unwrap();
        assert!(write(&path, Some(b"next")).is_err());
        assert_eq!(fs::read(temporary).unwrap(), b"unrelated");
        assert!(!path.exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn failed_suspend_resumes_without_modifying_settings() {
        let path = fixture("suspend");
        fs::write(&path, b"original").unwrap();
        let mut calls = Vec::new();
        assert!(
            change(&path, "normal", |resume| {
                calls.push(resume);
                if resume {
                    Ok(Value::Null)
                } else {
                    Err("suspend failed".into())
                }
            })
            .unwrap_err()
            .contains("suspend failed")
        );
        assert_eq!(calls, [false, true]);
        assert_eq!(fs::read(&path).unwrap(), b"original");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn serialized_growth_cannot_make_settings_unreadable() {
        let path = fixture("growth");
        let original =
            serde_json::to_vec(&json!({"version":1,"future":"x".repeat(LIMIT - 40)})).unwrap();
        assert!(original.len() <= LIMIT);
        fs::write(&path, &original).unwrap();
        let mut calls = Vec::new();
        let error = change(&path, "monitor-counterclockwise", |resume| {
            calls.push(resume);
            Ok(Value::Null)
        })
        .unwrap_err();
        assert!(error.contains("updated orientation settings exceed"));
        assert_eq!(calls, [false, true]);
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn symlink_and_oversized_settings_are_rejected_without_mutation() {
        let path = fixture("bounded");
        fs::write(&path, vec![b' '; LIMIT + 1]).unwrap();
        assert!(read(&path).unwrap_err().contains("exceed"));
        fs::remove_file(&path).unwrap();
        let target = path.with_extension("other");
        fs::write(&target, b"unrelated").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(read(&path).is_err());
        assert_eq!(fs::read(target).unwrap(), b"unrelated");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("orientation-{}-{name}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        root.join("settings.json")
    }

    #[test]
    fn all_orientations_preserve_other_settings_and_apply_after_suspend() {
        for orientation in ORIENTATIONS {
            let path = fixture(orientation);
            let original = br#"{"version":1,"reduce_motion":true,"future":{"value":42}}"#;
            fs::write(&path, original).unwrap();
            let mut calls = Vec::new();
            let result = change(&path, orientation, |resume| {
                calls.push(resume);
                if !resume {
                    assert_eq!(fs::read(&path).unwrap(), original);
                } else {
                    assert_eq!(
                        document(read(&path).unwrap().as_deref()).unwrap()["screen_orientation"],
                        orientation
                    );
                }
                Ok(json!({"launcher_ready_phase":"ready"}))
            })
            .unwrap();
            assert_eq!(calls, [false, true]);
            assert_eq!(result["configured_orientation"], orientation);
            let saved = document(read(&path).unwrap().as_deref()).unwrap();
            assert_eq!(saved["reduce_motion"], true);
            assert_eq!(saved["future"]["value"], 42);
            fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn activation_failure_restores_exact_settings_or_absence() {
        for present in [true, false] {
            let path = fixture(if present {
                "restore-present"
            } else {
                "restore-absent"
            });
            let original = br#"{ "version": 1, "reduce_motion": true }"#;
            if present {
                fs::write(&path, original).unwrap();
            }
            let mut calls = Vec::new();
            let error = change(&path, "monitor-clockwise", |resume| {
                calls.push(resume);
                if calls.len() == 2 {
                    return Err("activation failed".into());
                }
                Ok(Value::Null)
            })
            .unwrap_err();
            assert_eq!(calls, [false, true, false, true]);
            assert!(error.contains("orientation restoration: Ok"));
            assert_eq!(read(&path).unwrap(), present.then(|| original.to_vec()));
            fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn invalid_settings_are_preserved_and_launcher_is_resumed() {
        for (name, bytes) in [
            ("corrupt", b"broken".as_slice()),
            ("version", br#"{"version":2}"#),
            (
                "null-orientation",
                br#"{"version":1,"screen_orientation":null}"#,
            ),
            (
                "orientation",
                br#"{"version":1,"screen_orientation":"sideways"}"#,
            ),
        ] {
            let path = fixture(name);
            fs::write(&path, bytes).unwrap();
            let mut calls = Vec::new();
            assert!(
                change(&path, "normal", |resume| {
                    calls.push(resume);
                    Ok(Value::Null)
                })
                .is_err()
            );
            assert_eq!(calls, [false, true]);
            assert_eq!(fs::read(&path).unwrap(), bytes);
            fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }
}
