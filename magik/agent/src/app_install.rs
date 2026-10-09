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

pub const APP: &str = Layout::Development.paths().gui;
const ROOT: &str = Layout::Development.paths().root;
const MANIFEST: &str = Layout::Development.paths().manifest;
const OLD_APP: &str = "/media/fat/mister-magik-dev/.obsolete-app-delete-after-ready";
const OLD_MANIFEST: &str = "/media/fat/mister-magik-dev/.obsolete-manifest-delete-after-ready";

#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
enum InstallPhase {
    Preparing,
    Prepared,
    Restored,
    Ready,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct InstallState {
    phase: InstallPhase,
    expected_sha256: String,
    source_revision: String,
    source_dirty: Option<bool>,
    had_app: bool,
    had_env: bool,
    #[serde(default)]
    preserve_env_on_restore: bool,
}

struct InstallFiles {
    root: PathBuf,
}
impl InstallFiles {
    fn app(&self) -> PathBuf {
        self.root.join(Path::new(APP).file_name().unwrap())
    }
    fn manifest(&self) -> PathBuf {
        self.root.join(Path::new(MANIFEST).file_name().unwrap())
    }
    fn env(&self) -> PathBuf {
        self.root.join("launcher.env")
    }
    fn state_path(&self) -> PathBuf {
        self.root.join("app-install.json")
    }
    fn pairs(&self, state: &InstallState) -> Vec<(PathBuf, PathBuf)> {
        let mut pairs = vec![(
            self.manifest(),
            self.root.join(Path::new(OLD_MANIFEST).file_name().unwrap()),
        )];
        if state.had_app {
            pairs.push((
                self.app(),
                self.root.join(Path::new(OLD_APP).file_name().unwrap()),
            ));
        }
        if state.had_env {
            pairs.push((
                self.env(),
                self.root.join(".obsolete-env-delete-after-ready"),
            ));
        }
        pairs
    }
    fn load(&self) -> Result<Option<InstallState>, String> {
        match fs::read(self.state_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| error.to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
    fn save(&self, state: &InstallState) -> Result<(), String> {
        let temporary = self.root.join("app-install.next");
        fs::write(
            &temporary,
            serde_json::to_vec(state).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        File::open(&temporary)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        fs::rename(temporary, self.state_path()).map_err(|error| error.to_string())?;
        self.sync()
    }
    fn sync(&self) -> Result<(), String> {
        File::open(&self.root)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())
    }
    fn remove(&self, path: &Path) -> Result<(), String> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }
    fn cleanup_backups(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        for name in [
            Path::new(OLD_APP).file_name().unwrap(),
            Path::new(OLD_MANIFEST).file_name().unwrap(),
            std::ffi::OsStr::new(".obsolete-env-delete-after-ready"),
        ] {
            if let Err(error) = self.remove(&self.root.join(name)) {
                errors.push(error);
            }
        }
        if let Err(error) = self.sync() {
            errors.push(error);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
    fn restore(&self, state: &InstallState) -> Result<(), String> {
        if state.phase == InstallPhase::Ready {
            return self.cleanup_backups();
        }
        let mut errors = Vec::new();
        if state.phase == InstallPhase::Prepared {
            // Keep the backup sources until *all* restoration operations have
            // succeeded, so a failed rename can be retried after interruption.
            for (destination, backup) in self.pairs(state) {
                let result = (|| {
                    if destination == self.env() {
                        return managed_launcher::restore_managed_block(&destination, &backup);
                    }
                    let temporary = destination.with_extension("restore.next");
                    fs::copy(&backup, &temporary).map_err(|error| error.to_string())?;
                    File::open(&temporary)
                        .and_then(|file| file.sync_all())
                        .map_err(|error| error.to_string())?;
                    fs::rename(temporary, &destination).map_err(|error| error.to_string())
                })();
                if let Err(error) = result {
                    errors.push(format!("{}: {error}", destination.display()));
                }
            }
            if !state.had_app
                && let Err(error) = self.remove(&self.app())
            {
                errors.push(error);
            }
            if !state.had_env
                && !state.preserve_env_on_restore
                && let Err(error) = managed_launcher::update(&self.env(), None)
            {
                errors.push(error);
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        self.sync()?;
        let mut restored = state.clone();
        restored.phase = InstallPhase::Restored;
        self.save(&restored)?;
        self.cleanup_backups()?;
        self.remove(&self.state_path())?;
        self.sync()
    }
    fn published_matches(&self, state: &InstallState) -> bool {
        crate::installed_hash(&self.app()).as_deref() == Some(&state.expected_sha256)
            && fs::read_to_string(self.manifest())
                .ok()
                .and_then(|text| {
                    parse(&text, Layout::Development, ValidationProfile::AgentStrict).ok()
                })
                .is_some_and(|manifest| {
                    manifest.required("gui_sha256").ok() == Some(state.expected_sha256.as_str())
                })
    }
}

fn updated_manifest(
    text: &str,
    hash: &str,
    revision: &str,
    source_dirty: Option<bool>,
) -> Result<String, String> {
    let mut fields = parse(text, Layout::Development, ValidationProfile::AgentStrict)
        .map_err(|error| error.to_string())?
        .into_values();
    fields.insert("gui_sha256".into(), hash.into());
    // The manifest's revision field qualifies exact clean commits. Dirty
    // and unknown builds retain their base revision in the receipt instead.
    fields.insert(
        "magik_revision".into(),
        if source_dirty == Some(false) {
            revision.into()
        } else {
            "0".repeat(40)
        },
    );
    fields.insert(
        "qualification_candidate_id".into(),
        qualification_candidate_id(&fields),
    );
    let next = serialize(&fields).map_err(|error| error.to_string())?;
    parse(&next, Layout::Development, ValidationProfile::AgentStrict)
        .map_err(|error| error.to_string())?;
    Ok(next)
}

fn installation() -> InstallFiles {
    InstallFiles {
        root: PathBuf::from(ROOT),
    }
}

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
    running_for_state(&state)
}
fn running_for_state(state: &Value) -> Result<Value, String> {
    let Some(pid) = state["launcher_pid"].as_u64().filter(|pid| *pid > 0) else {
        return Ok(Value::Null);
    };
    let proc = PathBuf::from(format!("/proc/{pid}/exe"));
    match fs::read_link(&proc) {
        Ok(path) => Ok(json!({"pid":pid,"path":path,"sha256":crate::installed_hash(&proc)})),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(error) => Err(error.to_string()),
    }
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
            json!({"running":running().unwrap_or_else(|error|json!({"error":error})),"canonical":describe(Path::new(APP)),"installation_receipt":match installation().load() { Ok(state) => json!(state), Err(error) => json!({"error":error}) },
            "obsolete_copies":[describe(&self.install_root.join("magik")),
            describe(Path::new("/media/fat/mister-magik-dev/.mister-magik-fb.launch-return-canonical")),
            describe(Path::new("/media/fat/mister-magik-dev/.mister-magik-fb.upload")),describe(Path::new(OLD_APP))]}),
        )
    }
    fn quiesce_app(&self) -> Result<(), String> {
        let state = crate::device::status()?;
        let already_quiet = state["launcher_pid"].as_u64() == Some(0)
            && state["launcher_active"] == false
            && state["fpga_owner"] == "main"
            && matches!(
                state["launcher_state"].as_str(),
                Some("LauncherSuspended" | "Unconfigured")
            );
        if !already_quiet {
            crate::main_control::handoff("mister_magik_suspend\n")?;
        }
        self.stop_owned_process()?;
        let state = crate::device::status()?;
        if state["launcher_pid"].as_u64() != Some(0)
            || state["launcher_active"] != false
            || state["fpga_owner"] != "main"
        {
            return Err("Main has not quiesced the launcher; installation files retained".into());
        }
        Ok(())
    }
    pub(crate) fn recover_app_install(&self) -> Result<(), String> {
        managed_launcher::require_dev(&crate::device::status()?)?;
        self.recover_app_install_after_dev_start()
    }
    pub(crate) fn recover_app_install_after_dev_start(&self) -> Result<(), String> {
        crate::main_control::resume_after(
            || {
                managed_launcher::require_dev(&crate::device::status()?)?;
                let files = installation();
                let Some(state) = files.load()? else {
                    return self.recover_legacy_backups();
                };
                if state.phase == InstallPhase::Prepared
                    && files.published_matches(&state)
                    && self.app_is_verified_ready(&state.expected_sha256)?
                {
                    return self.finish_app_install(&state.expected_sha256);
                }
                if state.phase == InstallPhase::Ready {
                    return files.cleanup_backups();
                }
                self.quiesce_app()?;
                files.restore(&state)?;
                if state.preserve_env_on_restore {
                    managed_launcher::update(Path::new(managed_launcher::ENV_PATH), None)?;
                }
                Ok(())
            },
            || crate::main_control::handoff("mister_magik_resume\n"),
        )
    }
    fn recover_legacy_backups(&self) -> Result<(), String> {
        let files = installation();
        if !Path::new(OLD_APP).exists() && !Path::new(OLD_MANIFEST).exists() {
            return Ok(());
        }
        let old_manifest = fs::read_to_string(OLD_MANIFEST).ok().and_then(|text| {
            parse(&text, Layout::Development, ValidationProfile::AgentStrict).ok()
        });
        let old_is_complete = old_manifest.as_ref().is_some_and(|manifest| {
            manifest.required("gui_sha256").ok()
                == crate::installed_hash(Path::new(OLD_APP)).as_deref()
        });
        if !old_is_complete {
            let current = fs::read_to_string(MANIFEST).map_err(|error| error.to_string())?;
            let current = parse(
                &current,
                Layout::Development,
                ValidationProfile::AgentStrict,
            )
            .map_err(|error| error.to_string())?;
            if current.required("gui_sha256").ok()
                != crate::installed_hash(Path::new(APP)).as_deref()
            {
                return Err(
                    "legacy backups and installed app are inconsistent; no files changed".into(),
                );
            }
            return files.cleanup_backups();
        }
        // Upgrade the previous complete backup pair into a recoverable journal.
        let state = InstallState {
            phase: InstallPhase::Prepared,
            expected_sha256: String::new(),
            source_revision: String::new(),
            source_dirty: None,
            had_app: true,
            had_env: false,
            preserve_env_on_restore: true,
        };
        files.save(&state)?;
        self.quiesce_app()?;
        files.restore(&state)?;
        managed_launcher::update(Path::new(managed_launcher::ENV_PATH), None)
    }
    pub(crate) fn publish_app(
        &self,
        staged: &Staged,
        hash: &str,
        revision: &str,
        source_dirty: Option<bool>,
    ) -> Result<(), String> {
        managed_launcher::require_dev(&crate::device::status()?)?;
        if revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("canonical app upload requires an exact base source revision".into());
        }
        let files = installation();
        if files
            .root
            .canonicalize()
            .map_err(|error| error.to_string())?
            != files.root
        {
            return Err("Dev app directory must not redirect".into());
        }
        if let Some(state) = files.load()? {
            if matches!(state.phase, InstallPhase::Prepared | InstallPhase::Ready)
                && files.published_matches(&state)
                && state.expected_sha256 == hash
            {
                // Reconcile a lost upload response without overwriting its backups.
                return Ok(());
            }
            self.recover_app_install()?;
        } else {
            self.recover_app_install()?;
        }
        let text = fs::read_to_string(MANIFEST).map_err(|error| error.to_string())?;
        let next = updated_manifest(&text, hash, revision, source_dirty)?;
        let mut state = InstallState {
            phase: InstallPhase::Preparing,
            expected_sha256: hash.into(),
            source_revision: revision.into(),
            source_dirty,
            had_app: files.app().exists(),
            had_env: files.env().exists(),
            preserve_env_on_restore: false,
        };
        let result = (|| {
            self.quiesce_app()?;
            files.save(&state)?;
            let pairs = files.pairs(&state);
            let borrowed: Vec<_> = pairs
                .iter()
                .map(|(source, backup)| (source.as_path(), backup.as_path()))
                .collect();
            create_installation_backups(&borrowed, copy_installation_file, || Ok(()))?;
            state.phase = InstallPhase::Prepared;
            files.save(&state)?;
            staged.publish(&files.app())?;
            let temporary = files.manifest().with_extension("canonical.next");
            fs::write(&temporary, next).map_err(|error| error.to_string())?;
            File::open(&temporary)
                .and_then(|file| file.sync_all())
                .map_err(|error| error.to_string())?;
            fs::rename(temporary, files.manifest()).map_err(|error| error.to_string())?;
            files.sync()
        })();
        if let Err(error) = result {
            return crate::main_control::resume_after(
                || {
                    let recovered = files.load().and_then(|state| match state {
                        Some(state) => files.restore(&state),
                        None => Ok(()),
                    });
                    Err::<(), String>(match recovered {
                        Ok(()) => error,
                        Err(recovery) => format!("{error}; app restoration failed: {recovery}"),
                    })
                },
                || crate::main_control::handoff("mister_magik_resume\n"),
            );
        }
        Ok(())
    }
    fn app_is_verified_ready(&self, hash: &str) -> Result<bool, String> {
        let state = crate::device::status()?;
        let Some(pid) = state["launcher_pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
        else {
            return Ok(false);
        };
        let actual = running()?;
        Ok(actual["path"] == APP
            && actual["sha256"] == hash
            && crate::installed_hash(Path::new(APP)).as_deref() == Some(hash)
            && managed_launcher::owns_ready_child(&state, pid)
            && self.ready_for(pid, hash))
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
        let files = installation();
        if let Some(mut state) = files.load()? {
            if state.expected_sha256 != hash || !files.published_matches(&state) {
                return Err("installation receipt does not match the verified app".into());
            }
            state.phase = InstallPhase::Ready;
            files.save(&state)?;
        }
        files.cleanup_backups()?;
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
    const OLD_ENV: &[u8] = b"operator-settings\n# BEGIN magik managed launcher\nexport MISTER_MAGIK_PATH='/old'\n# END magik managed launcher\n";
    const NEW_ENV: &[u8] = b"operator-settings\n# BEGIN magik managed launcher\nexport MISTER_MAGIK_PATH='/new'\n# END magik managed launcher\n";
    fn valid_manifest(hash: &str) -> String {
        use mister_magik_platform_manifest_contract::{
            FORMAT, LATCH_CAPABILITY_MASK, LATCH_PROTOCOL_VERSION,
        };
        let mut values = std::collections::BTreeMap::from([
            ("format".into(), FORMAT.into()),
            ("platform_release".into(), "platform-v0.16".into()),
            ("platform_release_number".into(), "16".into()),
            ("platform_bundle_id".into(), "c".repeat(64)),
            (
                "latch_protocol_version".into(),
                LATCH_PROTOCOL_VERSION.to_string(),
            ),
            (
                "latch_capability_mask".into(),
                LATCH_CAPABILITY_MASK.to_string(),
            ),
            ("platform_contract_sha256".into(), "d".repeat(64)),
        ]);
        for (name, path) in Layout::Development.paths().components() {
            values.insert(format!("{name}_path"), path.into());
            values.insert(format!("{name}_sha256"), "a".repeat(64));
        }
        values.insert("gui_sha256".into(), hash.into());
        for field in ["main_revision", "magik_revision", "menu_revision"] {
            values.insert(field.into(), "b".repeat(40));
        }
        values.insert(
            "qualification_candidate_id".into(),
            qualification_candidate_id(&values),
        );
        serialize(&values).unwrap()
    }
    #[test]
    fn dirty_or_unknown_builds_do_not_qualify_the_base_commit_as_clean() {
        for dirty in [Some(false), Some(true), None] {
            let next = updated_manifest(
                &valid_manifest(&"a".repeat(64)),
                &"c".repeat(64),
                &"d".repeat(40),
                dirty,
            )
            .unwrap();
            let manifest =
                parse(&next, Layout::Development, ValidationProfile::AgentStrict).unwrap();
            assert_eq!(manifest.required("gui_sha256").unwrap(), "c".repeat(64));
            assert_eq!(
                manifest.required("magik_revision").unwrap(),
                if dirty == Some(false) {
                    "d".repeat(40)
                } else {
                    "0".repeat(40)
                }
            );
        }
    }
    #[test]
    fn lost_upload_reply_can_recognise_the_published_app_without_discarding_backups() {
        let (files, mut state) = interrupted_install("lost-reply", true);
        state.expected_sha256 = crate::installed_hash(&files.app()).unwrap();
        files.save(&state).unwrap();
        fs::write(files.manifest(), valid_manifest(&state.expected_sha256)).unwrap();
        let restarted = InstallFiles {
            root: files.root.clone(),
        };
        let recovered = restarted.load().unwrap().unwrap();
        assert!(restarted.published_matches(&recovered));
        assert!(
            files
                .pairs(&state)
                .iter()
                .all(|(_, backup)| backup.is_file())
        );
        fs::write(files.app(), b"different-app").unwrap();
        assert!(!restarted.published_matches(&recovered));
        fs::remove_dir_all(files.root).unwrap();
    }
    #[test]
    fn stopped_or_disappeared_launchers_are_inspectable() {
        for state in [
            json!({"launcher_pid":0}),
            json!({}),
            json!({"launcher_pid":u64::MAX}),
        ] {
            assert_eq!(running_for_state(&state).unwrap(), Value::Null);
        }
    }

    fn interrupted_install(label: &str, had_app: bool) -> (InstallFiles, InstallState) {
        let files = InstallFiles {
            root: std::env::temp_dir()
                .join(format!("magik-install-{label}-{}", std::process::id())),
        };
        fs::create_dir_all(&files.root).unwrap();
        if had_app {
            fs::write(files.app(), b"old-app").unwrap();
        }
        fs::write(files.manifest(), b"old-manifest").unwrap();
        fs::write(files.env(), OLD_ENV).unwrap();
        let mut state = InstallState {
            phase: InstallPhase::Preparing,
            expected_sha256: "a".repeat(64),
            source_revision: "b".repeat(40),
            source_dirty: Some(true),
            had_app,
            had_env: true,
            preserve_env_on_restore: false,
        };
        files.save(&state).unwrap();
        let pairs = files.pairs(&state);
        create_installation_backups(
            &pairs
                .iter()
                .map(|(source, backup)| (source.as_path(), backup.as_path()))
                .collect::<Vec<_>>(),
            copy_installation_file,
            || Ok(()),
        )
        .unwrap();
        state.phase = InstallPhase::Prepared;
        files.save(&state).unwrap();
        fs::write(files.app(), b"new-app").unwrap();
        fs::write(files.manifest(), b"new-manifest").unwrap();
        fs::write(files.env(), NEW_ENV).unwrap();
        (files, state)
    }

    #[test]
    fn service_restart_can_restore_interrupted_publication_or_first_install() {
        for had_app in [true, false] {
            let (files, _) =
                interrupted_install(if had_app { "replacement" } else { "first" }, had_app);
            let restarted = InstallFiles {
                root: files.root.clone(),
            };
            let state = restarted.load().unwrap().unwrap();
            assert_eq!(state.source_dirty, Some(true));
            restarted.restore(&state).unwrap();
            assert_eq!(fs::read(files.manifest()).unwrap(), b"old-manifest");
            assert_eq!(fs::read(files.env()).unwrap(), OLD_ENV);
            if had_app {
                assert_eq!(fs::read(files.app()).unwrap(), b"old-app");
            } else {
                assert!(!files.app().exists());
            }
            assert!(files.load().unwrap().is_none());
            assert!(
                files
                    .pairs(&state)
                    .iter()
                    .all(|(_, backup)| !backup.exists())
            );
            fs::remove_dir_all(files.root).unwrap();
        }
    }

    #[test]
    fn rollback_preserves_operator_edits_made_after_backup() {
        let (files, state) = interrupted_install("operator-edits", true);
        let current = std::str::from_utf8(NEW_ENV)
            .unwrap()
            .replace("operator-settings", "operator-edited");
        fs::write(files.env(), current).unwrap();
        files.restore(&state).unwrap();
        let expected = std::str::from_utf8(OLD_ENV)
            .unwrap()
            .replace("operator-settings", "operator-edited");
        assert_eq!(fs::read_to_string(files.env()).unwrap(), expected);
        fs::remove_dir_all(files.root).unwrap();
    }

    #[test]
    fn failed_rollback_retains_all_sources_and_retries_after_partial_restoration() {
        let (files, state) = interrupted_install("failed-restore", true);
        fs::remove_file(files.env()).unwrap();
        fs::create_dir(files.env()).unwrap();
        assert!(files.restore(&state).unwrap_err().contains("launcher.env"));
        assert_eq!(fs::read(files.app()).unwrap(), b"old-app");
        assert_eq!(fs::read(files.manifest()).unwrap(), b"old-manifest");
        assert!(
            files
                .pairs(&state)
                .iter()
                .all(|(_, backup)| backup.is_file())
        );
        fs::remove_dir(files.env()).unwrap();
        fs::write(files.env(), NEW_ENV).unwrap();
        files.restore(&files.load().unwrap().unwrap()).unwrap();
        assert_eq!(fs::read(files.env()).unwrap(), OLD_ENV);
        assert!(files.load().unwrap().is_none());
        fs::remove_dir_all(files.root).unwrap();
    }

    #[test]
    fn preparing_and_restored_journals_never_install_partial_backups() {
        for phase in [InstallPhase::Preparing, InstallPhase::Restored] {
            let (files, mut state) = interrupted_install(
                if phase == InstallPhase::Preparing {
                    "preparing"
                } else {
                    "restored"
                },
                true,
            );
            state.phase = phase;
            files.save(&state).unwrap();
            for (_, backup) in files.pairs(&state) {
                fs::write(backup, b"partial-backup").unwrap();
            }
            files.restore(&files.load().unwrap().unwrap()).unwrap();
            assert_eq!(fs::read(files.app()).unwrap(), b"new-app");
            assert_eq!(fs::read(files.manifest()).unwrap(), b"new-manifest");
            assert!(files.load().unwrap().is_none());
            fs::remove_dir_all(files.root).unwrap();
        }
    }

    #[test]
    fn ready_cleanup_failure_does_not_roll_back_the_verified_app() {
        let (files, mut state) = interrupted_install("ready-cleanup", true);
        state.phase = InstallPhase::Ready;
        files.save(&state).unwrap();
        let backup = files.pairs(&state)[0].1.clone();
        fs::remove_file(&backup).unwrap();
        fs::create_dir(&backup).unwrap();
        assert!(files.restore(&state).is_err());
        assert_eq!(fs::read(files.app()).unwrap(), b"new-app");
        assert_eq!(fs::read(files.manifest()).unwrap(), b"new-manifest");
        fs::remove_dir(backup).unwrap();
        files.restore(&files.load().unwrap().unwrap()).unwrap();
        assert_eq!(fs::read(files.app()).unwrap(), b"new-app");
        assert!(files.load().unwrap().unwrap().phase == InstallPhase::Ready);
        fs::remove_dir_all(files.root).unwrap();
    }

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
