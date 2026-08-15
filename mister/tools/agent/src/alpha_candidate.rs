// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Closed MiSTer Downloader transaction for the rolling alpha release.

use mister_magik_platform_manifest_contract::Layout;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CONFIG_PATH: &str = "/media/fat/downloader_mister_magik.ini";
const DOWNLOADER_ROOT: &str = "/media/fat";
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const DOWNLOADER_TIMEOUT: Duration = Duration::from_secs(240);
const DATABASE_ID: &str = "mister_magik";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallFailureKind {
    InvalidRequest,
    OperationFailed,
    ArtifactMismatch,
    RecoveryRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstallFailure {
    pub(crate) kind: InstallFailureKind,
    pub(crate) detail: String,
}

impl InstallFailure {
    fn invalid_request(detail: String) -> Self {
        Self {
            kind: InstallFailureKind::InvalidRequest,
            detail,
        }
    }

    fn operation(detail: String) -> Self {
        Self {
            kind: InstallFailureKind::OperationFailed,
            detail,
        }
    }

    fn artifact(detail: String) -> Self {
        Self {
            kind: InstallFailureKind::ArtifactMismatch,
            detail,
        }
    }

    fn recovery(detail: String) -> Self {
        Self {
            kind: InstallFailureKind::RecoveryRequired,
            detail,
        }
    }
}

pub(crate) fn install(args: Value) -> Result<Value, InstallFailure> {
    let request = Request::parse(&args).map_err(InstallFailure::invalid_request)?;
    require_single_canonical_section().map_err(InstallFailure::operation)?;
    let entrypoint = select_downloader().map_err(InstallFailure::operation)?;
    let mut config =
        ConfigTransaction::begin(Path::new(CONFIG_PATH)).map_err(InstallFailure::operation)?;
    let candidate_url = format!(
        "https://github.com/NigelBreslaw/MiSTer-MagiK/releases/download/{}/mister-magik-alpha-db.json.zip",
        request.tag
    );
    let install = (|| {
        config
            .replace(format!("[{DATABASE_ID}]\ndb_url = {candidate_url}\n").as_bytes())
            .map_err(InstallFailure::operation)?;
        run_downloader(&entrypoint).map_err(InstallFailure::operation)?;
        Ok::<(), InstallFailure>(())
    })();
    let restored = config.restore().map_err(InstallFailure::recovery);
    match (install, restored) {
        (Err(error), Ok(())) => return Err(error),
        (Ok(()), Err(error)) => {
            return Err(InstallFailure::recovery(format!(
                "candidate installed but config restore failed: {}",
                error.detail
            )));
        }
        (Err(error), Err(restore)) => {
            return Err(InstallFailure::recovery(format!(
                "{}; config restore also failed: {}",
                error.detail, restore.detail
            )));
        }
        (Ok(()), Ok(())) => {}
    }

    let installed = Layout::Public.paths();
    let manifest_path = Path::new(installed.manifest);
    require_hash(manifest_path, &request.platform_manifest).map_err(InstallFailure::artifact)?;
    for (name, path) in installed.components() {
        require_hash(
            Path::new(path),
            request
                .components
                .get(name)
                .ok_or_else(|| format!("missing expected {name} hash"))
                .map_err(InstallFailure::artifact)?,
        )
        .map_err(InstallFailure::artifact)?;
    }
    Ok(json!({
        "schema": "mister-magik-alpha-candidate-install-v1",
        "tag": request.tag,
        "database_id": DATABASE_ID,
        "downloader": entrypoint,
        "configuration_restored": true,
        "platform_manifest_sha256": request.platform_manifest,
    }))
}

struct Request {
    tag: String,
    platform_manifest: String,
    components: std::collections::BTreeMap<String, String>,
}

impl Request {
    fn parse(args: &Value) -> Result<Self, String> {
        let object = args
            .as_object()
            .ok_or("alpha candidate args must be an object")?;
        if object.len() != 3 {
            return Err("alpha candidate request has unknown or missing fields".to_string());
        }
        let tag = object
            .get("tag")
            .and_then(Value::as_str)
            .ok_or("alpha candidate tag is missing")?;
        validate_tag(tag)?;
        let platform_manifest = object
            .get("platform_manifest_sha256")
            .and_then(Value::as_str)
            .ok_or("platform manifest hash is missing")?;
        validate_sha(platform_manifest)?;
        let components = object
            .get("component_sha256")
            .and_then(Value::as_object)
            .ok_or("component hashes are missing")?;
        let installed = Layout::Public.paths().components();
        if components.len() != installed.len() {
            return Err("component hash set is incomplete".to_string());
        }
        let components = components
            .iter()
            .map(|(name, value)| {
                if !installed.iter().any(|(expected, _)| name == expected) {
                    return Err(format!("unsupported component hash: {name}"));
                }
                let hash = value
                    .as_str()
                    .ok_or_else(|| format!("component hash is not text: {name}"))?;
                validate_sha(hash)?;
                Ok((name.clone(), hash.to_string()))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            tag: tag.to_string(),
            platform_manifest: platform_manifest.to_string(),
            components,
        })
    }
}

fn validate_tag(tag: &str) -> Result<(), String> {
    if tag != "alpha" {
        return Err("alpha installation requires the rolling alpha tag".to_string());
    }
    Ok(())
}

fn validate_sha(value: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("candidate SHA-256 is invalid".to_string())
    }
}

fn require_single_canonical_section() -> Result<(), String> {
    require_single_canonical_section_at(Path::new(DOWNLOADER_ROOT), Path::new(CONFIG_PATH))
}

fn require_single_canonical_section_at(root: &Path, canonical: &Path) -> Result<(), String> {
    let mut owners = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name != "downloader.ini" && !(name.starts_with("downloader_") && name.ends_with(".ini"))
        {
            continue;
        }
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
            return Err(format!("unsafe Downloader configuration: {name}"));
        }
        let text = fs::read_to_string(entry.path()).map_err(|error| error.to_string())?;
        if text
            .lines()
            .any(|line| line.trim().eq_ignore_ascii_case("[mister_magik]"))
        {
            owners.push(entry.path());
        }
    }
    if !owners.is_empty() && owners.as_slice() != [canonical.to_path_buf()] {
        return Err(format!(
            "MiSTer MagiK Downloader section must have one canonical owner: {owners:?}"
        ));
    }
    Ok(())
}

fn select_downloader() -> Result<PathBuf, String> {
    for path in [
        "/media/fat/Scripts/update.sh",
        "/media/fat/Scripts/downloader.sh",
        "/media/fat/downloader.sh",
    ] {
        if fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
            return Ok(PathBuf::from(path));
        }
    }
    Err("official MiSTer Downloader entrypoint is missing".to_string())
}

fn run_downloader(entrypoint: &Path) -> Result<(), String> {
    let mut child = Command::new(entrypoint)
        .args(["--run-only", DATABASE_ID])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("start targeted Downloader: {error}"))?;
    let deadline = Instant::now() + DOWNLOADER_TIMEOUT;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("wait for targeted Downloader: {error}"))?
        {
            return if status.success() {
                Ok(())
            } else {
                Err(format!("targeted Downloader exited with {status}"))
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("targeted Downloader exceeded 240 seconds".to_string());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn require_hash(path: &Path, expected: &str) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let actual =
        Sha256::digest(bytes)
            .iter()
            .fold(String::with_capacity(64), |mut output, byte| {
                use std::fmt::Write as _;
                let _ = write!(output, "{byte:02x}");
                output
            });
    if actual == expected {
        Ok(())
    } else {
        Err(format!("installed hash mismatch: {}", path.display()))
    }
}

struct ConfigTransaction {
    path: PathBuf,
    original: Option<Vec<u8>>,
    mode: u32,
    replaced: bool,
}

impl ConfigTransaction {
    fn begin(path: &Path) -> Result<Self, String> {
        match fs::metadata(path) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
                    return Err("canonical Downloader configuration is unsafe".to_string());
                }
                Ok(Self {
                    path: path.to_path_buf(),
                    original: Some(fs::read(path).map_err(|error| error.to_string())?),
                    mode: metadata.permissions().mode(),
                    replaced: false,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                path: path.to_path_buf(),
                original: None,
                mode: 0o644,
                replaced: false,
            }),
            Err(error) => Err(error.to_string()),
        }
    }

    fn replace(&mut self, bytes: &[u8]) -> Result<(), String> {
        write_atomic(&self.path, bytes, self.mode)?;
        self.replaced = true;
        Ok(())
    }

    fn restore(&mut self) -> Result<(), String> {
        if self.replaced {
            if let Some(original) = self.original.as_deref() {
                write_atomic(&self.path, original, self.mode)?;
            } else {
                fs::remove_file(&self.path).map_err(|error| error.to_string())?;
                fs::File::open(self.path.parent().ok_or("config has no parent")?)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| error.to_string())?;
            }
            self.replaced = false;
        }
        Ok(())
    }
}

impl Drop for ConfigTransaction {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let temporary = path.with_file_name(format!(
        ".{}.alpha-candidate-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("downloader.ini"),
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(mode)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))
            .map_err(|error| error.to_string())?;
        fs::rename(&temporary, path).map_err(|error| error.to_string())?;
        fs::File::open(path.parent().ok_or("config has no parent")?)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(label: &str) -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mister-magik-alpha-candidate-{label}-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("create fixture");
        root
    }

    #[test]
    fn alpha_tag_is_closed_to_the_rolling_release() {
        assert!(validate_tag("alpha").is_ok());
        assert!(validate_tag("beta").is_err());
        assert!(validate_tag("alpha-old").is_err());
        assert!(validate_tag("alpha/escape").is_err());
    }

    #[test]
    fn absent_canonical_section_is_safe_for_a_transient_transaction() {
        let root = fixture("absent");
        let canonical = root.join("downloader_mister_magik.ini");
        assert!(require_single_canonical_section_at(&root, &canonical).is_ok());
        fs::remove_dir(root).expect("remove fixture");
    }

    #[test]
    fn alternate_section_owner_is_rejected() {
        let root = fixture("alternate-owner");
        let canonical = root.join("downloader_mister_magik.ini");
        fs::write(
            root.join("downloader_other.ini"),
            "[mister_magik]\ndb_url = https://example.invalid/db.zip\n",
        )
        .expect("write alternate owner");
        assert!(require_single_canonical_section_at(&root, &canonical).is_err());
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn transient_config_is_removed_on_restore() {
        let root = fixture("transient");
        let canonical = root.join("downloader_mister_magik.ini");
        let mut transaction = ConfigTransaction::begin(&canonical).expect("begin transaction");
        transaction
            .replace(b"[mister_magik]\ndb_url = https://example.invalid/db.zip\n")
            .expect("replace config");
        assert!(canonical.is_file());
        transaction.restore().expect("restore config");
        assert!(!canonical.exists());
        fs::remove_dir(root).expect("remove fixture");
    }

    #[test]
    fn canonical_config_without_a_section_is_restored_byte_for_byte() {
        let root = fixture("canonical-without-section");
        let canonical = root.join("downloader_mister_magik.ini");
        let original = b"[other]\ndb_url = https://example.invalid/other.zip\n";
        fs::write(&canonical, original).expect("write canonical config");
        assert!(require_single_canonical_section_at(&root, &canonical).is_ok());

        let mut transaction = ConfigTransaction::begin(&canonical).expect("begin transaction");
        transaction
            .replace(b"[mister_magik]\ndb_url = https://example.invalid/db.zip\n")
            .expect("replace config");
        transaction.restore().expect("restore config");
        assert_eq!(
            fs::read(&canonical).expect("read restored config"),
            original
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }
}
