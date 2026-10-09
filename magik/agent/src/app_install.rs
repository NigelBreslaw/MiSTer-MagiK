//! Fixed Dev app installation and verified removal of obsolete app copies.
use crate::{Agent, managed_launcher, upload::Staged};
use mister_magik_platform_manifest_contract::{
    Layout, ValidationProfile, parse, qualification_candidate_id, serialize,
};
use serde_json::{Value, json};
use std::{
    fs,
    fs::File,
    path::{Path, PathBuf},
};

pub const APP: &str = "/media/fat/mister-magik-dev/mister-magik-fb";
const ROOT: &str = "/media/fat/mister-magik-dev";
const MANIFEST: &str = "/media/fat/mister-magik-dev/platform-v3.manifest";
const OLD_APP: &str = "/media/fat/mister-magik-dev/.obsolete-app-delete-after-ready";
const OLD_MANIFEST: &str = "/media/fat/mister-magik-dev/.obsolete-manifest-delete-after-ready";

fn copy_installation_file(source: &Path, backup: &mut File) -> std::io::Result<u64> {
    let mut source = File::open(source)?;
    backup.set_permissions(source.metadata()?.permissions())?;
    std::io::copy(&mut source, backup)
}

fn create_installation_backups(
    files: &[(&Path, &Path)],
    mut copy: impl FnMut(&Path, &mut File) -> std::io::Result<u64>,
    resume_main: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let mut created = Vec::new();
    let result = (|| {
        for &(source, destination) in files {
            let mut backup = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)?;
            created.push(destination);
            copy(source, &mut backup)?;
            backup.sync_all()?;
        }
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = result {
        let mut cleanup_errors = Vec::new();
        for path in created {
            if let Err(cleanup) = fs::remove_file(path) {
                cleanup_errors.push(format!("{}: {cleanup}", path.display()));
            }
        }
        // Cleanup failures must never prevent Main from being resumed.
        let resumed = resume_main();
        return Err(format!(
            "backup creation failed: {error}; backup cleanup: {cleanup_errors:?}; Main resumption: {resumed:?}"
        ));
    }
    Ok(())
}
fn describe(path: &Path) -> Value {
    json!({"path":path,"exists":path.exists(),"sha256":crate::installed_hash(path)})
}
fn running() -> Result<Value, String> {
    let state = crate::device::status()?;
    let pid = state["launcher_pid"]
        .as_u64()
        .ok_or("launcher PID absent")?;
    let proc = PathBuf::from(format!("/proc/{pid}/exe"));
    Ok(
        json!({"pid":pid,"path":fs::read_link(&proc).map_err(|e|e.to_string())?,"sha256":crate::installed_hash(&proc)}),
    )
}
impl Agent {
    pub(crate) fn app_path(&self, artifact: &str) -> PathBuf {
        if artifact == "magik" {
            PathBuf::from(APP)
        } else {
            self.install_root.join(artifact)
        }
    }
    pub(crate) fn app_install_inspect(&self) -> Result<Value, String> {
        Ok(
            json!({"running":running()?,"canonical":describe(Path::new(APP)),
            "obsolete_copies":[describe(&self.install_root.join("magik")),
            describe(Path::new("/media/fat/mister-magik-dev/.mister-magik-fb.launch-return-canonical")),
            describe(Path::new("/media/fat/mister-magik-dev/.mister-magik-fb.upload")),describe(Path::new(OLD_APP))]}),
        )
    }
    pub(crate) fn publish_app(
        &self,
        staged: &Staged,
        hash: &str,
        revision: &str,
    ) -> Result<(), String> {
        managed_launcher::require_dev(&crate::device::status()?)?;
        if revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("canonical app upload requires an exact source revision".into());
        }
        if Path::new(ROOT).canonicalize().map_err(|e| e.to_string())? != Path::new(ROOT) {
            return Err("Dev app directory must not redirect".into());
        }
        if Path::new(OLD_APP).exists() || Path::new(OLD_MANIFEST).exists() {
            return Err("unfinished app installation must be reconciled first".into());
        }
        let text = fs::read_to_string(MANIFEST).map_err(|e| e.to_string())?;
        let mut fields = parse(&text, Layout::Development, ValidationProfile::AgentStrict)
            .map_err(|e| e.to_string())?
            .into_values();
        fields.insert("gui_sha256".into(), hash.into());
        fields.insert("magik_revision".into(), revision.into());
        fields.insert(
            "qualification_candidate_id".into(),
            qualification_candidate_id(&fields),
        );
        let next = serialize(&fields).map_err(|e| e.to_string())?;
        parse(&next, Layout::Development, ValidationProfile::AgentStrict)
            .map_err(|e| e.to_string())?;
        self.stop_owned_process()?;
        crate::main_control::handoff("mister_magik_suspend\n")?;
        create_installation_backups(
            &[
                (Path::new(APP), Path::new(OLD_APP)),
                (Path::new(MANIFEST), Path::new(OLD_MANIFEST)),
            ],
            copy_installation_file,
            || crate::main_control::handoff("mister_magik_resume\n"),
        )?;
        let result = (|| {
            staged.publish(Path::new(APP))?;
            let temporary = Path::new(MANIFEST).with_extension("canonical.next");
            fs::write(&temporary, next).map_err(|e| e.to_string())?;
            File::open(&temporary)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
            fs::rename(&temporary, MANIFEST).map_err(|e| e.to_string())?;
            File::open(ROOT)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            fs::rename(OLD_APP, APP).map_err(|e| e.to_string())?;
            fs::rename(OLD_MANIFEST, MANIFEST).map_err(|e| e.to_string())?;
            let _ = crate::main_control::handoff("mister_magik_resume\n");
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn restore_app_install(&self) -> Result<(), String> {
        if Path::new(OLD_APP).exists() && Path::new(OLD_MANIFEST).exists() {
            fs::rename(OLD_APP, APP).map_err(|e| e.to_string())?;
            fs::rename(OLD_MANIFEST, MANIFEST).map_err(|e| e.to_string())?;
            File::open(ROOT)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub(crate) fn finish_app_install(&self, hash: &str) -> Result<(), String> {
        let state = crate::device::status()?;
        let pid = state["launcher_pid"]
            .as_u64()
            .and_then(|p| u32::try_from(p).ok())
            .ok_or("launcher PID absent")?;
        let actual = running()?;
        if actual["path"] != APP
            || actual["sha256"] != hash
            || crate::installed_hash(Path::new(APP)).as_deref() != Some(hash)
            || !managed_launcher::owns_ready_child(&state, pid)
            || !self.ready_for(pid, hash)
        {
            return Err("latest canonical app is not verified running; old copies retained".into());
        }
        for path in [
            self.install_root.join("magik"),
            PathBuf::from(OLD_APP),
            PathBuf::from(OLD_MANIFEST),
            PathBuf::from("/media/fat/mister-magik-dev/.mister-magik-fb.launch-return-canonical"),
            PathBuf::from("/media/fat/mister-magik-dev/.mister-magik-fb.upload"),
        ] {
            match fs::symlink_metadata(&path) {
                Ok(m) if m.is_file() => fs::remove_file(&path).map_err(|e| e.to_string())?,
                Ok(_) => {
                    return Err(format!(
                        "obsolete app target is not a regular file: {}",
                        path.display()
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        File::open(ROOT)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn either_partial_backup_failure_cleans_up_and_resumes_without_changing_installation() {
        for fail_at in 0..2 {
            let root = std::env::temp_dir().join(format!(
                "magik-backup-failure-{}-{fail_at}",
                std::process::id()
            ));
            fs::create_dir_all(&root).unwrap();
            let app = root.join("app");
            let manifest = root.join("manifest");
            let old_app = root.join("old-app");
            let old_manifest = root.join("old-manifest");
            fs::write(&app, b"installed-app").unwrap();
            fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
            fs::write(&manifest, b"installed-manifest").unwrap();
            let mut copies = 0;
            let resumed = std::cell::Cell::new(false);
            let error = create_installation_backups(
                &[(&app, &old_app), (&manifest, &old_manifest)],
                |source, backup| {
                    let index = copies;
                    copies += 1;
                    if index == fail_at {
                        backup.write_all(b"partial")?;
                        return Err(std::io::Error::other("SD full"));
                    }
                    copy_installation_file(source, backup)
                },
                || {
                    resumed.set(true);
                    Ok(())
                },
            )
            .unwrap_err();
            assert!(error.contains("SD full"));
            assert!(resumed.get());
            assert!(!old_app.exists());
            assert!(!old_manifest.exists());
            assert_eq!(fs::read(&app).unwrap(), b"installed-app");
            assert_eq!(fs::read(&manifest).unwrap(), b"installed-manifest");
            // The cleaned installation accepts a subsequent backup attempt.
            create_installation_backups(
                &[(&app, &old_app), (&manifest, &old_manifest)],
                copy_installation_file,
                || panic!("successful backup must not resume Main"),
            )
            .unwrap();
            assert_eq!(
                fs::metadata(&old_app).unwrap().permissions().mode() & 0o777,
                0o755
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn backup_recovery_preserves_preexisting_files_and_reports_resume_failure() {
        let root =
            std::env::temp_dir().join(format!("magik-backup-existing-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("app");
        let destination = root.join("old-app");
        fs::write(&source, b"installed").unwrap();
        fs::write(&destination, b"preexisting").unwrap();
        let error = create_installation_backups(
            &[(&source, &destination)],
            |_, _| panic!("must not overwrite an existing backup"),
            || Err("Main unavailable".into()),
        )
        .unwrap_err();
        assert!(error.contains("Main unavailable"));
        assert_eq!(fs::read(destination).unwrap(), b"preexisting");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backup_cleanup_failure_still_resumes_main() {
        let root =
            std::env::temp_dir().join(format!("magik-backup-cleanup-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let destination = root.join("old-app");
        let resumed = std::cell::Cell::new(false);
        let error = create_installation_backups(
            &[(root.join("app").as_path(), &destination)],
            |_, _| {
                fs::remove_file(&destination)?;
                fs::create_dir(&destination)?;
                Err(std::io::Error::other("copy failed"))
            },
            || {
                resumed.set(true);
                Ok(())
            },
        )
        .unwrap_err();
        assert!(resumed.get());
        assert!(error.contains("backup cleanup:"));
        assert!(error.contains("old-app"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn real_app_resolves_to_canonical_dev_path_not_service_slot() {
        let root = std::env::temp_dir().join(format!("canonical-app-path-{}", std::process::id()));
        let agent = Agent::new("test".into(), "test-token".into(), root.clone());
        assert_eq!(agent.app_path("magik"), PathBuf::from(APP));
        assert_eq!(agent.app_path("mini-magik"), root.join("mini-magik"));
    }
}
