//! Headless Dev monitor orientation, using Main's owned launcher lifecycle.
use mister_magik_catalog::device_layout::DeviceLayout;
use mister_magik_mister_runtime::settings::{MagikSettings, ScreenOrientation};
use serde_json::{Value, json};
use std::ffi::{CString, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const LIMIT: usize = 64 * 1024;

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

fn document(bytes: Option<&[u8]>) -> Value {
    if let Some(mut value) = bytes.and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        && value.is_object()
        && value["version"] == 1
    {
        let orientation = value
            .as_object_mut()
            .expect("settings object")
            .remove("screen_orientation");
        // The launcher tolerates invalid orientation while retaining valid
        // other fields. If those fields are corrupt too, it uses defaults.
        if serde_json::from_value::<MagikSettings>(value.clone()).is_ok() {
            if let Some(orientation) = orientation {
                value["screen_orientation"] = orientation;
            }
            return value;
        }
    }
    serde_json::to_value(MagikSettings::default()).expect("default settings")
}

fn configured_orientation(value: &Value) -> ScreenOrientation {
    serde_json::from_value::<MagikSettings>(value.clone())
        .unwrap_or_default()
        .screen_orientation
}

fn write(path: &Path, bytes: Option<&[u8]>) -> Result<(), String> {
    let parent = path.parent().ok_or("settings parent missing")?;
    if let Some(bytes) = bytes {
        let mut template = CString::new(
            path.with_extension("orientation-XXXXXX")
                .as_os_str()
                .as_bytes(),
        )
        .map_err(|e| e.to_string())?
        .into_bytes_with_nul();
        // SAFETY: template is a writable NUL-terminated pathname ending in six Xs.
        // mkstemp exclusively creates a unique file with mode 0600.
        let descriptor = unsafe { libc::mkstemp(template.as_mut_ptr().cast()) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY: mkstemp returned a new owned descriptor; File closes it once.
        let mut file = unsafe { File::from_raw_fd(descriptor) };
        let temporary = PathBuf::from(OsString::from_vec(template[..template.len() - 1].to_vec()));
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
    orientation: ScreenOrientation,
    mut lifecycle: impl FnMut(bool) -> Result<Value, String>,
    mut persist: impl FnMut(&Path, Option<&[u8]>) -> Result<(), String>,
) -> Result<Value, String> {
    // Suspend before reading so no launcher writer can race this edit.
    let prepared = (|| {
        lifecycle(false)?;
        let original = read(path)?;
        let mut value = document(original.as_deref());
        value["screen_orientation"] = orientation.id().into();
        let next = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
        if next.len() > LIMIT {
            return Err("updated orientation settings exceed 64 KiB".into());
        }
        Ok::<_, String>((original, next))
    })();
    let (original, next) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            return crate::main_control::resume_after(
                || Err(error),
                || lifecycle(true).map(|_| ()),
            );
        }
    };
    let mut resume_attempted = false;
    let result = persist(path, Some(&next)).and_then(|()| {
        resume_attempted = true;
        lifecycle(true)
    });
    match result {
        Ok(state) => Ok(json!({"configured_orientation": orientation.id(), "main_status": state})),
        Err(error) => {
            // A write failure leaves Main suspended, even after rename. Only
            // a resume attempt can require suspending again before rollback.
            let restore = (|| {
                if resume_attempted {
                    lifecycle(false)?;
                }
                persist(path, original.as_deref())
            })();
            let restored =
                crate::main_control::resume_after(|| restore, || lifecycle(true).map(|_| ()));
            Err(format!("{error}; orientation restoration: {restored:?}"))
        }
    }
}

pub(super) fn status() -> Result<Value, String> {
    let state = crate::device::status()?;
    crate::managed_launcher::require_dev(&state)?;
    let path = DeviceLayout::Dev.app_path("settings.json");
    let value = document(read(&path)?.as_deref());
    Ok(json!({"configured_orientation": configured_orientation(&value).id(), "main_status": state}))
}

pub(super) fn set(fields: &serde_json::Map<String, Value>) -> Result<Value, String> {
    let orientation = fields
        .get("orientation")
        .and_then(Value::as_str)
        .unwrap_or("");
    let orientation = ScreenOrientation::ALL
        .into_iter()
        .find(|value| value.id() == orientation);
    if fields.len() != 2
        || fields.get("attended") != Some(&Value::Bool(true))
        || orientation.is_none()
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
    crate::managed_launcher::require_no_test_server(Path::new(crate::managed_launcher::ENV_PATH))?;
    change(
        &DeviceLayout::Dev.app_path("settings.json"),
        orientation.unwrap(),
        |resume| {
            if resume {
                crate::main_control::request("mister_magik_resume\n")?;
                crate::device::wait_launcher(None)
            } else {
                let state = crate::device::status()?;
                if state["launcher_state"] != "LauncherSuspended"
                    || state["launcher_active"] != false
                {
                    crate::main_control::handoff("mister_magik_suspend\n")?;
                }
                Ok(Value::Null)
            }
        },
        write,
    )
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
    fn stale_temporary_file_does_not_block_or_get_deleted() {
        let path = fixture("temporary");
        let temporary = path.with_extension(format!("orientation-{}.next", std::process::id()));
        fs::write(&temporary, b"unrelated").unwrap();
        let interrupted = path.with_extension("orientation-abc123");
        fs::write(&interrupted, b"interrupted").unwrap();
        write(&path, Some(b"next")).unwrap();
        write(&path, Some(b"again")).unwrap();
        assert_eq!(fs::read(temporary).unwrap(), b"unrelated");
        assert_eq!(fs::read(interrupted).unwrap(), b"interrupted");
        assert_eq!(fs::read(&path).unwrap(), b"again");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 3);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn failed_suspend_resumes_without_modifying_settings() {
        let path = fixture("suspend");
        fs::write(&path, b"original").unwrap();
        let mut calls = Vec::new();
        assert!(
            change(
                &path,
                ScreenOrientation::Normal,
                |resume| {
                    calls.push(resume);
                    if resume {
                        Ok(Value::Null)
                    } else {
                        Err("suspend failed".into())
                    }
                },
                write
            )
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
        let error = change(
            &path,
            ScreenOrientation::MonitorCounterclockwise,
            |resume| {
                calls.push(resume);
                Ok(Value::Null)
            },
            write,
        )
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
        for orientation in ScreenOrientation::ALL {
            let path = fixture(orientation.id());
            let original = br#"{"version":1,"reduce_motion":true,"future":{"value":42}}"#;
            fs::write(&path, original).unwrap();
            let mut calls = Vec::new();
            let result = change(
                &path,
                orientation,
                |resume| {
                    calls.push(resume);
                    if !resume {
                        assert_eq!(fs::read(&path).unwrap(), original);
                    } else {
                        assert_eq!(
                            document(read(&path).unwrap().as_deref())["screen_orientation"],
                            orientation.id()
                        );
                    }
                    Ok(json!({"launcher_ready_phase":"ready"}))
                },
                write,
            )
            .unwrap();
            assert_eq!(calls, [false, true]);
            assert_eq!(result["configured_orientation"], orientation.id());
            let saved = document(read(&path).unwrap().as_deref());
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
            let error = change(
                &path,
                ScreenOrientation::MonitorClockwise,
                |resume| {
                    calls.push(resume);
                    if calls.len() == 2 {
                        return Err("activation failed".into());
                    }
                    Ok(Value::Null)
                },
                write,
            )
            .unwrap_err();
            assert_eq!(calls, [false, true, false, true]);
            assert!(error.contains("orientation restoration: Ok"));
            assert_eq!(read(&path).unwrap(), present.then(|| original.to_vec()));
            fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn bad_saved_settings_can_be_repaired_with_launcher_defaults() {
        for (name, bytes) in [
            ("corrupt", b"broken".as_slice()),
            ("version", br#"{"version":2}"#),
            ("other-field", br#"{"version":1,"screen_orientation":"monitor-clockwise","reduce_motion":"broken","future":42}"#),
            ("null-orientation", br#"{"version":1,"screen_orientation":null,"reduce_motion":true,"future":42}"#),
            ("alias", br#"{"version":1,"screen_orientation":"clockwise","reduce_motion":true,"future":42}"#),
            ("orientation", br#"{"version":1,"screen_orientation":"sideways","reduce_motion":true,"future":42}"#),
        ] {
            let path = fixture(name);
            fs::write(&path, bytes).unwrap();
            assert_eq!(configured_orientation(&document(Some(bytes))), MagikSettings::load_from(&path).screen_orientation);
            let mut calls = Vec::new();
            change(&path, ScreenOrientation::MonitorClockwise, |resume| {
                calls.push(resume);
                Ok(Value::Null)
            }, write).unwrap();
            assert_eq!(calls, [false, true]);
            let saved = document(read(&path).unwrap().as_deref());
            assert_eq!(configured_orientation(&saved), ScreenOrientation::MonitorClockwise);
            assert_eq!(MagikSettings::load_from(&path).screen_orientation, ScreenOrientation::MonitorClockwise);
            if !matches!(name, "corrupt" | "version" | "other-field") {
                assert_eq!(saved["reduce_motion"], true);
                assert_eq!(saved["future"], 42);
            } else {
                assert_eq!(saved["reduce_motion"], false);
            }
            fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn write_failure_after_rename_restores_without_resuspending() {
        for present in [true, false] {
            let path = fixture(if present {
                "write-failure-present"
            } else {
                "write-failure-absent"
            });
            let original = br#"{ "version": 1, "reduce_motion": true }"#;
            if present {
                fs::write(&path, original).unwrap();
            }
            let mut calls = Vec::new();
            let mut writes = 0;
            let error = change(
                &path,
                ScreenOrientation::MonitorClockwise,
                |resume| {
                    calls.push(resume);
                    // Main rejects a second suspend while already suspended.
                    assert_eq!(
                        calls,
                        if resume {
                            vec![false, true]
                        } else {
                            vec![false]
                        }
                    );
                    Ok(Value::Null)
                },
                |path, bytes| {
                    writes += 1;
                    write(path, bytes)?;
                    if writes == 1 {
                        Err("directory sync failed after rename".into())
                    } else {
                        Ok(())
                    }
                },
            )
            .unwrap_err();
            assert!(error.contains("directory sync failed after rename"));
            assert!(error.contains("orientation restoration: Ok"));
            assert_eq!(read(&path).unwrap(), present.then(|| original.to_vec()));
            assert_eq!(calls, [false, true]);
            fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn failed_repair_restores_corrupt_bytes_exactly() {
        let path = fixture("restore-corrupt");
        fs::write(&path, b"corrupt original").unwrap();
        let mut calls = 0;
        let error = change(
            &path,
            ScreenOrientation::MonitorClockwise,
            |_| {
                calls += 1;
                if calls == 2 {
                    Err("activation failed".into())
                } else {
                    Ok(Value::Null)
                }
            },
            write,
        )
        .unwrap_err();
        assert!(error.contains("orientation restoration: Ok"));
        assert_eq!(fs::read(&path).unwrap(), b"corrupt original");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn restoration_and_resumption_failures_are_both_reported() {
        let path = fixture("failed-restoration");
        let mut calls = Vec::new();
        let error = change(
            &path,
            ScreenOrientation::Normal,
            |resume| {
                calls.push(resume);
                if resume {
                    Err("resume failed".into())
                } else {
                    Ok(Value::Null)
                }
            },
            |_, _| Err("write failed".into()),
        )
        .unwrap_err();
        assert_eq!(calls, [false, true]);
        assert!(error.contains("write failed"));
        assert!(error.contains("Main resumption failed: resume failed"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
