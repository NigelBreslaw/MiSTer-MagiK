// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::device::DeviceClient;
use crate::error::AgentResult;
use crate::progress::{EventKind, Reporter};
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const MAIN_BUILD_DEADLINE: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalMainExecution {
    pub app_revision: String,
    pub main_revision: String,
    pub main_sha256: String,
    pub qualification_candidate_id: String,
}

trait LocalMainDevice {
    fn connect(&mut self) -> AgentResult<()>;
    fn verify_development_platform(&mut self) -> AgentResult<()>;
    fn read_development_manifest(&mut self) -> AgentResult<String>;
    fn deliver_local_main(&mut self, delivery: LocalMainDelivery) -> AgentResult<()>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocalMainDelivery {
    local: PathBuf,
    manifest_local: PathBuf,
    expected_main_sha256: String,
    expected_gui_sha256: String,
}

impl LocalMainDevice for DeviceClient {
    fn connect(&mut self) -> AgentResult<()> {
        self.read(crate::NativeDevice::discover)
    }

    fn verify_development_platform(&mut self) -> AgentResult<()> {
        self.read(crate::NativeDevice::verify_development_platform)
    }

    fn read_development_manifest(&mut self) -> AgentResult<String> {
        self.read(crate::NativeDevice::read_development_manifest)
    }

    fn deliver_local_main(&mut self, delivery: LocalMainDelivery) -> AgentResult<()> {
        self.mutate(|device| {
            device.deliver_local_main(
                &delivery.local,
                &delivery.manifest_local,
                &delivery.expected_main_sha256,
                &delivery.expected_gui_sha256,
            )
        })
    }
}

pub fn execute(
    repository: &Path,
    app_revision: &str,
    reporter: &mut Reporter<'_>,
) -> AgentResult<LocalMainExecution> {
    let main = configured_main_repository(repository)?;
    require_clean_revision(repository, app_revision, "app")?;
    let main_revision = crate::git::value(&main, &["rev-parse", "HEAD"])?;
    require_clean_revision(&main, &main_revision, "Main")?;

    reporter.emit(
        EventKind::Progress,
        "platform-identity",
        "verifying the coherent installed Dev platform",
        Some(5),
    )?;
    verify_installed_dev_platform(DeviceClient::default())?;

    reporter.emit(
        EventKind::Progress,
        "main-validation",
        "validating the clean local Main patch surface",
        Some(10),
    )?;
    run_bounded(&main, "scripts/test-magik-state.sh", &[])?;
    run_bounded(&main, "scripts/check-magik-patch-surface.sh", &[])?;
    require_clean_revision(&main, &main_revision, "Main")?;

    reporter.emit(
        EventKind::Progress,
        "main-build",
        "building the exact local Main commit",
        Some(35),
    )?;
    run_bounded(&main, "./build-container.sh", &["clean", "all"])?;
    require_clean_revision(repository, app_revision, "app")?;
    require_clean_revision(&main, &main_revision, "Main")?;
    let artifact = main.join("bin/MiSTer");
    validate_arm_main(&artifact)?;

    reporter.emit(
        EventKind::Progress,
        "platform-identity",
        "verifying the installed Dev platform before overlay generation",
        Some(65),
    )?;
    execute_overlay_transaction(
        repository,
        &main_revision,
        &artifact,
        DeviceClient::default(),
    )
}

fn verify_installed_dev_platform(mut device: impl LocalMainDevice) -> AgentResult<String> {
    device.connect()?;
    device.verify_development_platform()?;
    let installed = device.read_development_manifest()?;
    let installed = crate::platform_manifest::parse_installed(
        &installed,
        crate::platform_manifest::Layout::Development,
    )?;
    Ok(installed.magik_revision().into())
}

fn execute_overlay_transaction(
    repository: &Path,
    main_revision: &str,
    artifact: &Path,
    mut device: impl LocalMainDevice,
) -> AgentResult<LocalMainExecution> {
    device.connect()?;
    device.verify_development_platform()?;
    let installed = device.read_development_manifest()?;
    let installed_identity = crate::platform_manifest::parse_installed(
        &installed,
        crate::platform_manifest::Layout::Development,
    )?;
    let installed_app_revision = installed_identity.magik_revision().to_string();
    let stage = repository
        .join("build/agent-deploy/local-main")
        .join(format!("{installed_app_revision}-{main_revision}"));
    if stage.exists() {
        fs::remove_dir_all(&stage)
            .map_err(|error| format!("cannot clear {}: {error}", stage.display()))?;
    }
    fs::create_dir_all(&stage)
        .map_err(|error| format!("cannot create {}: {error}", stage.display()))?;
    let manifest = stage.join(crate::platform_manifest::FILE_NAME);
    let identity = crate::platform_manifest::write_local_main_overlay(
        &manifest,
        &installed,
        artifact,
        main_revision,
        &installed_app_revision,
    )?;
    device.deliver_local_main(LocalMainDelivery {
        local: artifact.to_path_buf(),
        manifest_local: manifest,
        expected_main_sha256: identity.main_sha256.clone(),
        expected_gui_sha256: identity.gui_sha256,
    })?;
    Ok(LocalMainExecution {
        app_revision: installed_app_revision,
        main_revision: main_revision.into(),
        main_sha256: identity.main_sha256,
        qualification_candidate_id: identity.qualification_candidate_id,
    })
}

fn configured_main_repository(repository: &Path) -> AgentResult<PathBuf> {
    let configured = env::var_os("MISTER_MAIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            repository
                .parent()
                .unwrap_or(repository)
                .join("Main_MiSTer")
        });
    if !configured.join(".git").exists() || !configured.join("build-container.sh").is_file() {
        return Err(format!(
            "local Main repository is unavailable or incomplete: {}",
            configured.display()
        )
        .into());
    }
    Ok(configured)
}

fn require_clean_revision(repository: &Path, expected: &str, label: &str) -> AgentResult<()> {
    let head = crate::git::value(repository, &["rev-parse", "HEAD"])?;
    if head != expected {
        return Err(format!(
            "{label} source identity changed during local Main delivery: expected={expected} actual={head}"
        )
        .into());
    }
    let dirty = crate::git::value(repository, &["status", "--porcelain"])?;
    if !dirty.is_empty() {
        return Err(format!("{label} worktree must be clean for local Main delivery").into());
    }
    Ok(())
}

fn validate_arm_main(path: &Path) -> AgentResult<()> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("local Main artifact is missing {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() < 20 {
        return Err(format!("local Main artifact is invalid: {}", path.display()).into());
    }
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!("local Main artifact is not executable: {}", path.display()).into());
    }
    let mut header = [0_u8; 20];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|error| format!("cannot read local Main ELF header: {error}"))?;
    let machine = u16::from_le_bytes([header[18], header[19]]);
    if &header[..4] != b"\x7fELF" || header[4] != 1 || header[5] != 1 || machine != 40 {
        return Err(format!(
            "local Main artifact is not a 32-bit little-endian ARM ELF: {}",
            path.display()
        )
        .into());
    }
    Ok(())
}

fn run_bounded(repository: &Path, program: &str, args: &[&str]) -> AgentResult<()> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(repository)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("cannot start {program}: {error}"))?;
    let status =
        crate::process::wait(&mut child, Some(MAIN_BUILD_DEADLINE), program, None, || {
            Ok(())
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}").into())
    }
}

#[cfg(test)]
fn test_manifest(magik_revision: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;

    let fields = crate::platform_manifest::FIELDS;
    let mut values = BTreeMap::new();
    values.insert(
        "format".to_owned(),
        crate::platform_manifest::FORMAT.to_owned(),
    );
    values.insert("platform_release".to_owned(), "platform-v0.16".to_owned());
    values.insert("platform_release_number".to_owned(), "16".to_owned());
    values.insert("platform_bundle_id".to_owned(), "c".repeat(64));
    values.insert("latch_protocol_version".to_owned(), "5".to_owned());
    values.insert("latch_capability_mask".to_owned(), "0x03ff".to_owned());
    for (name, path) in crate::platform_manifest::Layout::Development
        .paths()
        .components()
    {
        values.insert(format!("{name}_path"), path.into());
    }
    for name in [
        "main",
        "gui",
        "manager",
        "scanout_module",
        "scanout_metadata",
        "latch_rbf",
        "latch_metadata",
        "platform_contract",
    ] {
        values.insert(format!("{name}_sha256"), "d".repeat(64));
    }
    values.insert("main_revision".to_owned(), "e".repeat(40));
    values.insert("magik_revision".to_owned(), magik_revision.into());
    values.insert("menu_revision".to_owned(), "f".repeat(40));
    let mut hash = Sha256::new();
    for field in fields {
        if *field != "qualification_candidate_id" {
            hash.update(field.as_bytes());
            hash.update(b"=");
            hash.update(values[*field].as_bytes());
            hash.update(b"\n");
        }
    }
    values.insert(
        "qualification_candidate_id".to_owned(),
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    );
    fields
        .iter()
        .map(|field| format!("{field}={}\n", values[*field]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct FakeLocalMainDevice {
        manifest: String,
        deliveries: Rc<Cell<usize>>,
    }

    impl LocalMainDevice for FakeLocalMainDevice {
        fn connect(&mut self) -> AgentResult<()> {
            Ok(())
        }

        fn verify_development_platform(&mut self) -> AgentResult<()> {
            Ok(())
        }

        fn read_development_manifest(&mut self) -> AgentResult<String> {
            Ok(self.manifest.clone())
        }

        fn deliver_local_main(&mut self, _delivery: LocalMainDelivery) -> AgentResult<()> {
            self.deliveries.set(self.deliveries.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn arm_validation_rejects_non_arm_and_accepts_arm_elf32() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-local-main-elf-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let artifact = root.join("MiSTer");
        fs::write(&artifact, b"not an ELF artifact").unwrap();
        let mut permissions = fs::metadata(&artifact).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&artifact, permissions).unwrap();
        assert!(validate_arm_main(&artifact).is_err());
        let mut elf = [0_u8; 20];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 1;
        elf[5] = 1;
        elf[18..20].copy_from_slice(&40_u16.to_le_bytes());
        fs::write(&artifact, elf).unwrap();
        assert!(validate_arm_main(&artifact).is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn local_main_overlay_uses_one_typed_mutation_after_read_only_verification() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-local-main-device-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let artifact = root.join("MiSTer");
        fs::write(&artifact, b"main").unwrap();
        let manifest = super::test_manifest(&"a".repeat(40));
        let deliveries = Rc::new(Cell::new(0));
        let fake = FakeLocalMainDevice {
            manifest,
            deliveries: Rc::clone(&deliveries),
        };
        let execution =
            execute_overlay_transaction(&root, &"b".repeat(40), &artifact, fake).unwrap();
        assert_eq!(execution.app_revision, "a".repeat(40));
        assert_eq!(execution.main_revision, "b".repeat(40));
        assert_eq!(deliveries.get(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn installed_app_revision_is_preserved_independently_of_host_head() {
        let fake = FakeLocalMainDevice {
            manifest: super::test_manifest(&"a".repeat(40)),
            deliveries: Rc::new(Cell::new(0)),
        };
        let revision = verify_installed_dev_platform(fake).unwrap();
        assert_eq!(revision, "a".repeat(40));
    }
}
