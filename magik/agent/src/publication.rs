//! Fixed artifact sets, verified staging and reversible publication. No shell.
use crate::{Envelope, FrameError, response, write_frame};
use mister_magik_platform_manifest_contract::{Layout, ValidationProfile, parse};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::Read;
use std::net::TcpStream;
use std::path::{Path, PathBuf};

const PLATFORM: &[&str] = &[
    "main",
    "gui",
    "manager",
    "scanout_module",
    "scanout_metadata",
    "latch_rbf",
    "latch_metadata",
    "manifest",
];
const DATABASES: &[&str] = &[
    "magik-metadata-v1.bin",
    "arcade-updater-index-v1.lz4b",
    "game-databases-SHA256SUMS",
    "game-databases-manifest.json",
];

fn installed_state(fat: &Path, install_root: &Path) -> Result<Value, String> {
    let paths = Layout::Development.paths();
    let local = |path: &str| fat.join(path.strip_prefix("/media/fat/").expect("layout path"));
    let mut platform = json!({"version":0,"verified":false});
    let manifest_path = local(paths.manifest);
    if manifest_path.exists() {
        let text = fs::read_to_string(manifest_path).map_err(|e| e.to_string())?;
        let manifest = parse(&text, Layout::Development, ValidationProfile::AgentStrict)
            .map_err(|e| e.to_string())?;
        let mut hashes = serde_json::Map::new();
        let mut verified = true;
        for (name, path) in paths.components() {
            let hash = crate::media::hash(&local(path)).ok();
            if !matches!(name, "gui" | "manager") {
                verified &= hash.as_deref()
                    == Some(
                        manifest
                            .required(&format!("{name}_sha256"))
                            .map_err(|e| e.to_string())?,
                    );
            }
            hashes.insert(name.to_owned(), json!(hash));
        }
        platform = json!({"version":manifest.required("platform_release_number").map_err(|e| e.to_string())?.parse::<u64>().map_err(|e| e.to_string())?,
            "bundle_id":manifest.required("platform_bundle_id").map_err(|e| e.to_string())?,"verified":verified,"hashes":hashes});
    }
    let assets = local(paths.root).join("assets");
    let mut databases = json!({"version":0,"verified":false});
    let manifest_path = assets.join("game-databases-manifest.json");
    if manifest_path.exists() {
        let manifest: Value =
            serde_json::from_slice(&fs::read(manifest_path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let checks =
            fs::read_to_string(assets.join("game-databases-SHA256SUMS")).unwrap_or_default();
        let mut hashes = serde_json::Map::new();
        let mut verified = manifest["format"] == "mister-magik-game-databases-manifest-v4";
        for name in &DATABASES[..2] {
            let hash = crate::media::hash(&assets.join(name)).ok();
            let expected = checks
                .lines()
                .filter_map(|line| line.split_once("  "))
                .find_map(|(hash, path)| (path == *name).then_some(hash));
            verified &= expected.is_some() && hash.as_deref() == expected;
            hashes.insert((*name).to_owned(), json!(hash));
        }
        hashes.insert(
            "manifest".into(),
            json!(crate::media::hash(
                &assets.join("game-databases-manifest.json")
            )?),
        );
        databases =
            json!({"version":manifest["release_version"],"verified":verified,"hashes":hashes});
    }
    let mut stages = Vec::new();
    let publication = install_root.join("publication");
    if publication.exists() {
        for entry in fs::read_dir(publication).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let pending = entry.path().join("pending.json");
            if pending.exists() || entry.path().join("backup").exists() {
                let detail = if pending.exists() {
                    serde_json::from_slice::<Value>(&fs::read(pending).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                } else {
                    Value::Null
                };
                stages.push(json!({"stage":entry.file_name().to_string_lossy(),"pending":detail}));
            }
        }
    }
    Ok(json!({"platform":platform,"databases":databases,"stages":stages}))
}

fn stage(root: &Path, fields: &serde_json::Map<String, Value>) -> Result<PathBuf, String> {
    let id = fields
        .get("stage")
        .and_then(Value::as_str)
        .ok_or("publication stage required")?;
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid publication stage".into());
    }
    Ok(root.join("publication").join(id))
}

pub fn receive(
    root: &Path,
    request: &Envelope,
    reader: &mut impl Read,
    length: usize,
) -> Result<Value, String> {
    if request.fields.len() != 3 {
        return Err("publication upload requires stage, artifact and sha256".into());
    }
    let artifact = request
        .fields
        .get("artifact")
        .and_then(Value::as_str)
        .ok_or("artifact required")?;
    if !PLATFORM.contains(&artifact) && !DATABASES.contains(&artifact) {
        return Err("unsupported publication artifact".into());
    }
    let root = stage(root, &request.fields)?;
    let hash = request
        .fields
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or("sha256 required")?;
    let staged = crate::upload::receive(reader, &root, "publication", hash, length, &request.id)?;
    staged.publish(&root.join(artifact))?;
    Ok(json!({"stage":request.fields["stage"],"artifact":artifact,"sha256":hash}))
}

fn files(
    root: &Path,
    fields: &serde_json::Map<String, Value>,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let staged = stage(root, fields)?;
    let layout = Layout::parse(
        fields
            .get("layout")
            .and_then(Value::as_str)
            .ok_or("layout required")?,
    )
    .map_err(|e| e.to_string())?;
    let kind = fields
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("publication kind required")?;
    let paths = layout.paths();
    if kind == "databases" {
        // Uploaded files have each been hashed before publication. The manifest
        // must identify the compact format used by the installed application.
        let report: Value = serde_json::from_slice(
            &fs::read(staged.join("game-databases-manifest.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if report["format"] != "mister-magik-game-databases-manifest-v4" {
            return Err("database release must use compact runtime metadata".into());
        }
        let checks = fs::read_to_string(staged.join("game-databases-SHA256SUMS"))
            .map_err(|e| e.to_string())?;
        for name in &DATABASES[..2] {
            let expected = checks
                .lines()
                .find_map(|line| {
                    let (hash, path) = line.split_once("  ")?;
                    (path == *name).then_some(hash)
                })
                .ok_or("database checksum missing")?;
            if crate::media::hash(&staged.join(name))? != expected {
                return Err(format!("database checksum mismatch: {name}"));
            }
        }
        return Ok(DATABASES
            .iter()
            .map(|name| {
                (
                    staged.join(name),
                    Path::new(paths.root).join("assets").join(name),
                )
            })
            .collect());
    }
    if !matches!(kind, "platform" | "local-main" | "fpga")
        || fields.get("attended") != Some(&Value::Bool(true))
    {
        return Err(
            "platform publication requires an attended platform or local-main request".into(),
        );
    }
    if matches!(kind, "local-main" | "fpga") && layout != Layout::Development {
        return Err("local Main delivery is Dev-only".into());
    }
    if matches!(kind, "platform" | "fpga")
        && fields.get("activate_fpga") != Some(&Value::Bool(true))
    {
        return Err("platform delivery requires explicit FPGA activation".into());
    }
    let text = fs::read_to_string(staged.join("manifest")).map_err(|e| e.to_string())?;
    let manifest =
        parse(&text, layout, ValidationProfile::AgentStrict).map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for (name, path) in paths.components() {
        let source = if (kind == "local-main" && name != "main")
            || (kind == "fpga" && !matches!(name, "latch_rbf" | "latch_metadata"))
        {
            PathBuf::from(path)
        } else {
            staged.join(name)
        };
        if crate::media::hash(&source)?
            != manifest
                .required(&format!("{name}_sha256"))
                .map_err(|e| e.to_string())?
        {
            return Err(format!("publication artifact mismatch: {name}"));
        }
        if kind == "platform"
            || (kind == "local-main" && name == "main")
            || (kind == "fpga" && matches!(name, "latch_rbf" | "latch_metadata"))
        {
            result.push((source, PathBuf::from(path)));
        }
    }
    // Publish the binding manifest last.
    result.push((staged.join("manifest"), PathBuf::from(paths.manifest)));
    Ok(result)
}

fn replace(
    files: &[(PathBuf, PathBuf)],
    backup: &Path,
    activate: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    fs::create_dir_all(backup).map_err(|e| e.to_string())?;
    File::open(backup.parent().ok_or("backup parent missing")?)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    if backup.join("transaction.json").exists() {
        return Err("publication transaction already exists; reconcile it explicitly".into());
    }
    let mut applied = Vec::new();
    let result = (|| {
        for (index, (source, destination)) in files.iter().enumerate() {
            let parent = destination.parent().ok_or("publication parent absent")?;
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            let saved = backup.join(index.to_string());
            let existed = destination.try_exists().map_err(|e| e.to_string())?;
            if existed {
                fs::copy(destination, &saved).map_err(|e| e.to_string())?;
                File::open(&saved)
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
            }
            // Persist recovery information before replacing any installed file.
            applied.push((destination.clone(), saved, existed));
            save_journal(backup, &applied)?;
            // Rename requires a common filesystem and leaves no partial file.
            fs::rename(source, destination).map_err(|e| e.to_string())?;
            File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
        }
        activate()
    })();
    if let Err(error) = result {
        let restored = restore(backup);
        return Err(format!("{error}; artifact restoration: {restored:?}"));
    }
    Ok(())
}

fn save_journal(backup: &Path, entries: &[(PathBuf, PathBuf, bool)]) -> Result<(), String> {
    use std::io::Write;
    let temporary = backup.join("transaction.part");
    let mut file = File::create(&temporary).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(entries).map_err(|e| e.to_string())?)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(temporary, backup.join("transaction.json")).map_err(|e| e.to_string())?;
    File::open(backup)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

fn restore(backup: &Path) -> Result<(), String> {
    let entries: Vec<(PathBuf, PathBuf, bool)> = serde_json::from_slice(
        &fs::read(backup.join("transaction.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut failures = Vec::new();
    for (destination, saved, existed) in entries.into_iter().rev() {
        let result = if existed {
            // Keep backups so an interrupted restoration can be repeated explicitly.
            let temporary = destination.with_extension("magik-restore");
            fs::copy(&saved, &temporary)
                .and_then(|_| File::open(&temporary)?.sync_all())
                .and_then(|()| fs::rename(&temporary, &destination))
        } else {
            match fs::remove_file(&destination) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            }
        };
        if let Err(e) = result.and_then(|()| File::open(destination.parent().unwrap())?.sync_all())
        {
            failures.push(format!("{}: {e}", destination.display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn reload_healthy(previous: &Value, expected: &str) -> Result<(), String> {
    // Main accepts supervised reload only while its Dev launcher is active.
    crate::main_control::handoff("mister_magik_resume\n")?;
    crate::main_control::request("mister_magik_reload_main\n")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let state = crate::device::status()?;
        if state["main_generation"] != previous["main_generation"]
            && state["launcher_ready_phase"] == "ready"
        {
            let pid = state["pid"].as_u64().ok_or("Main PID missing")?;
            if crate::media::hash(Path::new(&format!("/proc/{pid}/exe")))? == expected {
                return Ok(());
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err("Main reload did not become healthy with the intended artifact".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

impl crate::Agent {
    pub(super) fn publication_state(
        &self,
        stream: &mut TcpStream,
        request: &Envelope,
    ) -> Result<(), FrameError> {
        let _mutation = self.mutations.lock().expect("mutation state poisoned");
        let result = (|| -> Result<Value, String> {
            if request.fields.len() != 1 || request.fields["layout"] != "dev" {
                return Err("publication-state requires layout=dev".into());
            }
            let mut state = installed_state(Path::new("/media/fat"), &self.install_root)?;
            state["running"] = crate::device::status()?;
            let paths = Layout::Development.paths();
            let running_hash = state["running"]["pid"]
                .as_u64()
                .and_then(|pid| crate::media::hash(Path::new(&format!("/proc/{pid}/exe"))).ok());
            state["platform"]["active"] = json!(
                state["running"]["executable_path"] == paths.main
                    && state["running"]["launcher_ready_phase"] == "ready"
                    && state["running"]["scanout_slots_module_loaded"] == true
                    && running_hash.is_some()
                    && running_hash == crate::media::hash(Path::new(paths.main)).ok()
                    && state["stages"]
                        .as_array()
                        .is_some_and(|stages| stages.is_empty())
            );
            state["configured_main"] = crate::mode::status()?["configured_main"].clone();
            state["boot_id"] = json!(
                fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(|e| e.to_string())?
            );
            Ok(state)
        })();
        let reply = match result {
            Ok(fields) => response(&request.id, "publication-state", fields),
            Err(error) => response(
                &request.id,
                "error",
                json!({"code":"publication-state-failed","detail":error}),
            ),
        };
        write_frame(stream, &reply, &[])
    }

    pub(super) fn publication_control(
        &self,
        stream: &mut TcpStream,
        request: &Envelope,
    ) -> Result<(), FrameError> {
        let _mutation = self.mutations.lock().expect("mutation state poisoned");
        let result = (|| -> Result<Value, String> {
            if request.fields.len() != 3
                || request.fields.get("attended") != Some(&Value::Bool(true))
            {
                return Err("publication control requires stage, action and attendance".into());
            }
            let root = stage(&self.install_root, &request.fields)?;
            let pending: Value = serde_json::from_slice(
                &fs::read(root.join("pending.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            match request.fields.get("action").and_then(Value::as_str) {
                Some("finish") => {
                    if pending["kind"] == "databases" {
                        let fields = json!({"stage":request.fields["stage"],"kind":"databases","layout":pending["layout"]});
                        for (source, destination) in
                            files(&self.install_root, fields.as_object().unwrap())?
                        {
                            if crate::media::hash(&source)? != crate::media::hash(&destination)? {
                                return Err(
                                    "database publication is incomplete; restore explicitly".into(),
                                );
                            }
                        }
                        fs::remove_dir_all(root).map_err(|e| e.to_string())?;
                        return Ok(json!({"activated":true}));
                    }
                    if pending["boot_id"]
                        == fs::read_to_string("/proc/sys/kernel/random/boot_id")
                            .map_err(|e| e.to_string())?
                    {
                        return Err("platform activation requires a confirmed new boot".into());
                    }
                    let layout = pending["layout"].as_str().ok_or("pending layout absent")?;
                    crate::mode::verify_platform(layout)?;
                    let state = crate::device::status()?;
                    let layout = Layout::parse(layout).map_err(|e| e.to_string())?;
                    let expected = pending["manifest_sha256"].as_str().ok_or(
                        "pending publication has no expected manifest; restore explicitly",
                    )?;
                    if crate::media::hash(Path::new(layout.paths().manifest))? != expected {
                        return Err("activated manifest differs from staged publication".into());
                    }
                    let pid = state["pid"].as_u64().ok_or("Main PID missing")?;
                    if crate::media::hash(Path::new(&format!("/proc/{pid}/exe")))?
                        != crate::media::hash(Path::new(layout.paths().main))?
                    {
                        return Err("running Main differs from installed Main".into());
                    }
                    if state["executable_path"] != layout.paths().main
                        || state["launcher_ready_phase"] != "ready"
                        || state["scanout_slots_module_loaded"] != true
                    {
                        return Err("intended platform is not healthy".into());
                    }
                    fs::remove_dir_all(root).map_err(|e| e.to_string())?;
                    Ok(json!({"activated":true}))
                }
                Some("restore") => {
                    self.stop_owned_process()?;
                    if let Err(error) = crate::main_control::handoff("mister_magik_suspend\n") {
                        let restored = crate::main_control::handoff("mister_magik_resume\n");
                        return Err(format!("{error}; Main restoration: {restored:?}"));
                    }
                    let restored = restore(&root.join("backup"));
                    let resumed = crate::main_control::handoff("mister_magik_resume\n");
                    restored?;
                    resumed?;
                    Ok(json!({"restored":true,"requires_explicit_reboot":true}))
                }
                _ => Err("unknown publication control action".into()),
            }
        })();
        let reply = match result {
            Ok(value) => response(&request.id, "publication-complete", value),
            Err(error) => response(
                &request.id,
                "error",
                json!({"code":"publication-reconciliation-failed","detail":error}),
            ),
        };
        write_frame(stream, &reply, &[])
    }

    pub(super) fn publish(
        &self,
        stream: &mut TcpStream,
        request: &Envelope,
    ) -> Result<(), FrameError> {
        let _mutation = self.mutations.lock().expect("mutation state poisoned");
        let result = (|| {
            let fields = &request.fields;
            if fields.keys().any(|key| {
                !["stage", "kind", "layout", "attended", "activate_fpga"].contains(&key.as_str())
            }) {
                return Err("unexpected publication fields".into());
            }
            let paths = files(&self.install_root, fields)?;
            let platform = fields["kind"] != "databases";
            let previous = crate::device::status()?;
            if platform {
                let layout = Layout::parse(fields["layout"].as_str().ok_or("layout required")?)
                    .map_err(|e| e.to_string())?;
                if previous["executable_path"] != layout.paths().main {
                    return Err("platform publication layout differs from running Main; select the intended mode explicitly first".into());
                }
            }
            if fields["kind"] == "platform" {
                let selected = crate::mode::status()?["configured_main"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                let running = previous["executable_path"].as_str().unwrap_or("");
                if Path::new(running).file_name().and_then(|v| v.to_str())
                    != Some(selected.as_str())
                {
                    return Err("configured boot mode differs from the intended platform; select it explicitly before delivery".into());
                }
            }
            let original_main = if platform {
                Some(crate::media::hash(Path::new(
                    previous["executable_path"]
                        .as_str()
                        .ok_or("Main executable absent")?,
                ))?)
            } else {
                None
            };
            let expected_main = if platform {
                let layout = Layout::parse(fields["layout"].as_str().ok_or("layout required")?)
                    .map_err(|e| e.to_string())?;
                let source = paths
                    .iter()
                    .find(|(_, path)| path == Path::new(layout.paths().main))
                    .map(|(source, _)| source.as_path())
                    .unwrap_or(Path::new(layout.paths().main));
                Some(crate::media::hash(source)?)
            } else {
                None
            };
            let root = stage(&self.install_root, fields)?;
            let requires_reboot = fields["kind"] == "platform";
            if requires_reboot || fields["kind"] == "databases" {
                let expected_manifest = if requires_reboot {
                    "manifest"
                } else {
                    "game-databases-manifest.json"
                };
                let pending = json!({"kind":fields["kind"],"layout":fields["layout"],"manifest_sha256":crate::media::hash(&root.join(expected_manifest))?,"boot_id":fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(|e|e.to_string())?});
                fs::write(
                    root.join("pending.json"),
                    serde_json::to_vec(&pending).unwrap(),
                )
                .map_err(|e| e.to_string())?;
                File::open(root.join("pending.json"))
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
                File::open(&root)
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
            }
            if platform {
                crate::mode::disarm()?;
            }
            self.stop_owned_process()?;
            if let Err(error) = crate::main_control::handoff("mister_magik_suspend\n") {
                let restored = crate::main_control::handoff("mister_magik_resume\n");
                return Err(format!("{error}; Main restoration: {restored:?}"));
            }
            let published = replace(&paths, &root.join("backup"), || {
                if platform && !requires_reboot {
                    reload_healthy(&previous, expected_main.as_deref().unwrap())
                } else {
                    // Full platform activation includes the kernel module: the host
                    // performs one explicit reboot, then confirms this transaction.
                    crate::main_control::handoff("mister_magik_resume\n")
                }
            });
            if let Err(error) = published {
                let restored = if platform && !requires_reboot {
                    let current = crate::device::status().unwrap_or(previous.clone());
                    reload_healthy(&current, original_main.as_deref().unwrap())
                } else {
                    crate::main_control::handoff("mister_magik_resume\n")
                };
                return Err(format!(
                    "{error}; Main restoration: {restored:?}; staging retained at {}",
                    root.display()
                ));
            }
            if !requires_reboot {
                fs::remove_dir_all(&root).map_err(|e| e.to_string())?;
            }
            Ok(
                json!({"kind":fields["kind"],"layout":fields["layout"],"files":paths.len(),"main_restored":true,"requires_reboot":requires_reboot}),
            )
        })();
        let reply = match result {
            Ok(value) => response(&request.id, "publication-complete", value),
            Err(error) => response(
                &request.id,
                "error",
                json!({"code":"publication-failed","detail":error}),
            ),
        };
        write_frame(stream, &reply, &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_state_hashes_databases_and_retains_interrupted_stage() {
        let root =
            std::env::temp_dir().join(format!("magik2-publication-state-{}", std::process::id()));
        let fat = root.join("fat");
        let assets = fat.join("mister-magik-dev/assets");
        let install = root.join("agent");
        fs::create_dir_all(&assets).unwrap();
        let mut checks = String::new();
        for name in &DATABASES[..2] {
            fs::write(assets.join(name), b"content").unwrap();
            checks.push_str(&format!(
                "{}  {name}\n",
                crate::media::hash(&assets.join(name)).unwrap()
            ));
        }
        fs::write(assets.join("game-databases-SHA256SUMS"), checks).unwrap();
        fs::write(
            assets.join("game-databases-manifest.json"),
            br#"{"format":"mister-magik-game-databases-manifest-v4","release_version":4}"#,
        )
        .unwrap();
        let stage = install.join("publication").join("a".repeat(32));
        fs::create_dir_all(stage.join("backup")).unwrap();
        let state = installed_state(&fat, &install).unwrap();
        assert_eq!(state["databases"]["verified"], true);
        assert_eq!(state["databases"]["version"], 4);
        assert_eq!(state["stages"].as_array().unwrap().len(), 1);
        assert!(stage.join("backup").exists());
        fs::write(assets.join(DATABASES[0]), b"corrupt").unwrap();
        assert_eq!(
            installed_state(&fat, &install).unwrap()["databases"]["verified"],
            false
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_activation_restores_original_artifact_set() {
        let root = std::env::temp_dir().join(format!("magik-publish-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("new");
        let destination = root.join("installed");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();
        assert!(
            replace(
                &[(source, destination.clone())],
                &root.join("backup"),
                || Err("health failed".into())
            )
            .is_err()
        );
        assert_eq!(fs::read(destination).unwrap(), b"old");
        fs::remove_dir_all(root).unwrap();
    }
}
