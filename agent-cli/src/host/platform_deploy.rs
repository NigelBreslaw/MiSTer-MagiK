// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::installed_layout::{app_path, arming_paths, paths};
use super::{
    DeliveryTransferMetrics, DeployRemote, ExecOutput, Layout, Path, PathBuf, Result, Session,
    SshDeployRemote, exec_failure_message, file_sha256, fs, put_measured, sh, shell_sequence,
};

const SNES_ARTWORK_SHA256: &str =
    "7a76993e7e1b0063832b94e9d2ad588549587cf09a14ac2ced72d349ed12f766";

pub(super) fn installed_platform_verify_command(layout: Layout) -> String {
    let installed = paths(layout);
    let root = installed.root;
    let main = installed.main;
    format!(
        "set -eu; root={root}; manifest=$root/platform-v3.manifest; test -s \"$manifest\"; test -x {main}; test -x \"$root/mister-magik-fb\"; test -x \"$root/mister-magik-manager\"; test -r \"$root/mister_magik_scanout_slots.ko\"; test -r \"$root/fpga/menu-magik-vblank-latch.rbf\"; test -r \"$root/assets/snes/snes-small-v1.rgb565a\"; grep -qx 'format={manifest_format}' \"$manifest\"; get() {{ sed -n \"s/^$1=//p\" \"$manifest\"; }}; test \"$(sha256sum {main} | awk '{{print $1}}')\" = \"$(get main_sha256)\"; test \"$(sha256sum \"$root/mister-magik-fb\" | awk '{{print $1}}')\" = \"$(get gui_sha256)\"; test \"$(sha256sum \"$root/mister-magik-manager\" | awk '{{print $1}}')\" = \"$(get manager_sha256)\"; test \"$(sha256sum \"$root/mister_magik_scanout_slots.ko\" | awk '{{print $1}}')\" = \"$(get scanout_module_sha256)\"; test \"$(sha256sum \"$root/fpga/menu-magik-vblank-latch.rbf\" | awk '{{print $1}}')\" = \"$(get latch_rbf_sha256)\"; test \"$(sha256sum \"$root/assets/snes/snes-small-v1.rgb565a\" | awk '{{print $1}}')\" = {SNES_ARTWORK_SHA256}",
        manifest_format = crate::platform_manifest::FORMAT,
    )
}

pub(super) fn platform_deploy_files() -> Vec<(&'static str, String)> {
    let installed = paths(Layout::Development);
    let mut files = vec![
        ("mister-magik-fb", installed.gui.to_owned()),
        ("mister-magik-manager", installed.manager.to_owned()),
        ("MiSTer_MagiKDev", installed.main.to_owned()),
        (
            "mister_magik_scanout_slots.ko",
            installed.scanout_module.to_owned(),
        ),
        (
            "mister_magik_scanout_slots.metadata.txt",
            installed.scanout_metadata.to_owned(),
        ),
        (
            "fpga/menu-magik-vblank-latch.rbf",
            installed.latch_rbf.to_owned(),
        ),
        (
            "fpga/menu-magik-vblank-latch.metadata.txt",
            installed.latch_metadata.to_owned(),
        ),
        (
            "assets/snes/snes-small-v1.rgb565a",
            app_path(Layout::Development, "assets/snes/snes-small-v1.rgb565a")
                .expect("static installed path"),
        ),
        (
            "platform-v3.manifest",
            app_path(Layout::Development, "platform-v3.manifest").expect("static installed path"),
        ),
    ];
    files.splice(7..7, database_deploy_files());
    files
}

pub(super) fn database_deploy_files() -> Vec<(&'static str, String)> {
    [
        "mame.sqlite3",
        "hbmame.sqlite3",
        "arcade-updater-index-v1.lz4b",
        "game-databases-SHA256SUMS",
        "game-databases-manifest.json",
    ]
    .into_iter()
    .map(|name| {
        (
            name,
            app_path(Layout::Development, name).expect("static installed path"),
        )
    })
    .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PlatformDeployTransaction {
    pub(super) stage: PathBuf,
    pub(super) files: Vec<PlatformDeployFile>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DatabaseDeployTransaction(PlatformDeployTransaction);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PlatformDeployFile {
    pub(super) local: PathBuf,
    pub(super) remote: String,
    pub(super) sha256: String,
    pub(super) bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PlatformDeployReport {
    pub(super) changed_files: usize,
    pub(super) skipped_files: usize,
    pub(super) transferred_bytes: u64,
    pub(super) transfer_ms: u64,
}

impl PlatformDeployTransaction {
    pub(super) fn validate(stage: &Path) -> Result<Self> {
        Self::validate_files(stage, platform_deploy_files(), "platform")
    }

    fn validate_files(
        stage: &Path,
        deploy_files: Vec<(&'static str, String)>,
        label: &str,
    ) -> Result<Self> {
        if !stage.is_dir() {
            return Err(format!("{label} stage is missing: {}", stage.display()).into());
        }
        let mut files = Vec::new();
        for (relative, remote) in deploy_files {
            let local = stage.join(relative);
            if !local.is_file() {
                return Err(format!("{label} stage is missing {relative}").into());
            }
            files.push(PlatformDeployFile {
                bytes: fs::metadata(&local)?.len(),
                sha256: file_sha256(local.clone())?,
                local,
                remote,
            });
        }
        Ok(Self {
            stage: stage.to_path_buf(),
            files,
        })
    }

    pub(super) fn run(
        &self,
        sess: &Session,
        metrics: &mut DeliveryTransferMetrics,
    ) -> Result<PlatformDeployReport> {
        self.run_with(
            &SshDeployRemote {
                sess,
                remote_host: None,
            },
            metrics,
        )
    }

    pub(super) fn run_with<R: DeployRemote>(
        &self,
        remote: &R,
        metrics: &mut DeliveryTransferMetrics,
    ) -> Result<PlatformDeployReport> {
        let inventory = remote.exec(&self.inventory_command())?;
        if let Some(message) = exec_failure_message("platform inventory", &inventory) {
            return Err(message.into());
        }
        let installed = self.parse_inventory(&inventory.stdout)?;
        let changed = self
            .files
            .iter()
            .zip(installed)
            .filter_map(|(file, installed)| {
                (installed.as_deref() != Some(&file.sha256)).then_some(file)
            })
            .collect::<Vec<_>>();
        let report = PlatformDeployReport {
            changed_files: changed.len(),
            skipped_files: self.files.len().saturating_sub(changed.len()),
            transferred_bytes: changed.iter().map(|file| file.bytes).sum(),
            transfer_ms: 0,
        };
        if changed.is_empty() {
            println!(
                "platform deploy ok stage={} changed_files=0 skipped_files={} transferred_bytes=0 transfer_ms=0",
                self.stage.display(),
                report.skipped_files,
            );
            return Ok(report);
        }

        let fpga = app_path(Layout::Development, "fpga").expect("static installed path");
        let snapshots = app_path(Layout::Development, "snapshots").expect("static installed path");
        remote
            .exec(&format!("mkdir -p {} {}", sh(&fpga), sh(&snapshots)))
            .and_then(|output| checked_deploy_output("platform prepare", output))?;
        let transfer_before = metrics.upload_ms;
        for file in &changed {
            put_measured(
                remote,
                &file.local,
                &format!("{}.upload", file.remote),
                file.bytes,
                metrics,
            )?;
        }
        let mut report = report;
        report.transfer_ms = metrics.upload_ms.saturating_sub(transfer_before);
        let script = self.activation_script(&changed);
        let output = remote.exec(&script)?;
        if let Some(message) = exec_failure_message("platform activation", &output) {
            return Err(message.into());
        }
        println!(
            "platform deploy ok stage={} changed_files={} skipped_files={} transferred_bytes={} transfer_ms={}",
            self.stage.display(),
            report.changed_files,
            report.skipped_files,
            report.transferred_bytes,
            report.transfer_ms,
        );
        Ok(report)
    }

    fn inventory_command(&self) -> String {
        let mut command = String::from("set -eu; ");
        for file in &self.files {
            if let Some(name) = file
                .local
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| {
                    matches!(
                        *name,
                        "mame.sqlite3"
                            | "hbmame.sqlite3"
                            | "arcade-updater-index-v1.lz4b"
                            | "game-databases-manifest.json"
                    )
                })
            {
                let sums = app_path(Layout::Development, "game-databases-SHA256SUMS")
                    .expect("static installed path");
                command.push_str(&format!(
                    "sum=$(awk '$2 == \"{name}\" {{print $1}}' {sums} 2>/dev/null || true); if test -n \"$sum\"; then printf '%s  {path}\\n' \"$sum\"; else printf 'missing  {path}\\n'; fi; ",
                    sums = sh(&sums),
                    path = file.remote,
                ));
                continue;
            }
            command.push_str(&format!(
                "if test -f {path}; then sha256sum {path}; else printf 'missing  %s\\n' {path}; fi; ",
                path = sh(&file.remote),
            ));
        }
        command
    }

    fn parse_inventory(&self, stdout: &str) -> Result<Vec<Option<String>>> {
        let lines = stdout.lines().collect::<Vec<_>>();
        if lines.len() != self.files.len() {
            return Err(format!(
                "platform inventory returned {} lines for {} files",
                lines.len(),
                self.files.len()
            )
            .into());
        }
        lines
            .into_iter()
            .zip(&self.files)
            .map(|(line, file)| {
                let mut fields = line.split_whitespace();
                let fingerprint = fields.next().unwrap_or_default();
                let path = fields.next().unwrap_or_default();
                if path != file.remote {
                    return Err(format!(
                        "platform inventory path mismatch: expected {} got {}",
                        file.remote, path
                    )
                    .into());
                }
                if fingerprint == "missing" {
                    return Ok(None);
                }
                if fingerprint.len() != 64
                    || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(
                        format!("platform inventory invalid SHA-256 for {}", file.remote).into(),
                    );
                }
                Ok(Some(fingerprint.to_ascii_lowercase()))
            })
            .collect()
    }

    pub(super) fn activation_script(&self, changed: &[&PlatformDeployFile]) -> String {
        let mut verify = String::new();
        let mut activate = String::new();
        let mut rollback = String::new();
        for file in changed {
            verify.push_str(&format!(
                "actual=$(sha256sum {} | awk '{{print $1}}'); if test \"$actual\" != {}; then printf 'platform upload hash mismatch: {} expected={} actual=%s\\n' \"$actual\" >&2; exit 1; fi; ",
                sh(&format!("{}.upload", file.remote)),
                sh(&file.sha256),
                sh(&file.remote),
                sh(&file.sha256),
            ));
            rollback.push_str(&format!(
                "if [ -e {backup} ]; then mv -f {backup} {path}; elif [ -e {missing} ]; then rm -f {path} {missing}; fi; ",
                path = sh(&file.remote),
                backup = sh(&format!("{}.rollback", file.remote)),
                missing = sh(&format!("{}.rollback-missing", file.remote))
            ));
        }
        for file in changed
            .iter()
            .filter(|file| !file.remote.ends_with("platform-v3.manifest"))
        {
            activate.push_str(&format!(
                "mv -f {} {}; ",
                sh(&format!("{}.upload", file.remote)),
                sh(&file.remote)
            ));
        }
        if let Some(manifest) = changed
            .iter()
            .find(|file| file.remote.ends_with("platform-v3.manifest"))
        {
            activate.push_str(&format!(
                "mv -f {} {}; ",
                sh(&format!("{}.upload", manifest.remote)),
                sh(&manifest.remote)
            ));
        }
        let mut chmod = String::new();
        for file in changed.iter().filter(|file| {
            file.remote.ends_with("/mister-magik-fb")
                || file.remote.ends_with("/mister-magik-manager")
                || file.remote == paths(Layout::Development).main
        }) {
            chmod.push_str(&format!("chmod 755 {}; ", sh(&file.remote)));
        }
        let safety = platform_safety_script();
        let finish = shell_sequence([safety.as_str(), "trap - EXIT INT TERM", "sync"]);
        let require_snapshot = "if ! test -f /media/fat/MiSTer.ini.platform-rollback; then printf 'platform snapshot missing: /media/fat/MiSTer.ini.platform-rollback\\n' >&2; exit 1; fi";
        format!(
            "set -eu; {safety}; {require_snapshot}; {verify} rollback() {{ {rollback} mv -f /media/fat/MiSTer.ini.platform-rollback /media/fat/MiSTer.ini 2>/dev/null || true; sync; }}; trap rollback EXIT INT TERM; {activate} {chmod} sync; {finish}"
        )
    }
}

impl DatabaseDeployTransaction {
    pub(super) fn validate(stage: &Path) -> Result<Self> {
        PlatformDeployTransaction::validate_files(stage, database_deploy_files(), "database")
            .map(Self)
    }

    pub(super) fn run(
        &self,
        sess: &Session,
        metrics: &mut DeliveryTransferMetrics,
    ) -> Result<PlatformDeployReport> {
        self.run_with(
            &SshDeployRemote {
                sess,
                remote_host: None,
            },
            metrics,
        )
    }

    fn run_with<R: DeployRemote>(
        &self,
        remote: &R,
        metrics: &mut DeliveryTransferMetrics,
    ) -> Result<PlatformDeployReport> {
        let transaction = &self.0;
        let inventory = remote.exec(&transaction.inventory_command())?;
        if let Some(message) = exec_failure_message("database inventory", &inventory) {
            return Err(message.into());
        }
        let installed = transaction.parse_inventory(&inventory.stdout)?;
        let changed = transaction
            .files
            .iter()
            .zip(installed)
            .filter_map(|(file, installed)| {
                (installed.as_deref() != Some(&file.sha256)).then_some(file)
            })
            .collect::<Vec<_>>();
        let mut report = PlatformDeployReport {
            changed_files: changed.len(),
            skipped_files: transaction.files.len().saturating_sub(changed.len()),
            transferred_bytes: changed.iter().map(|file| file.bytes).sum(),
            transfer_ms: 0,
        };
        if changed.is_empty() {
            println!(
                "database deploy ok stage={} changed_files=0 skipped_files={} transferred_bytes=0 transfer_ms=0",
                transaction.stage.display(),
                report.skipped_files,
            );
            return Ok(report);
        }

        let transfer_before = metrics.upload_ms;
        for file in &changed {
            put_measured(
                remote,
                &file.local,
                &format!("{}.upload", file.remote),
                file.bytes,
                metrics,
            )?;
        }
        report.transfer_ms = metrics.upload_ms.saturating_sub(transfer_before);
        let output = remote.exec(&database_activation_script(&changed))?;
        if let Some(message) = exec_failure_message("database activation", &output) {
            return Err(message.into());
        }
        println!(
            "database deploy ok stage={} changed_files={} skipped_files={} transferred_bytes={} transfer_ms={}",
            transaction.stage.display(),
            report.changed_files,
            report.skipped_files,
            report.transferred_bytes,
            report.transfer_ms,
        );
        Ok(report)
    }
}

fn database_activation_script(changed: &[&PlatformDeployFile]) -> String {
    let mut snapshot = String::new();
    let mut verify = String::new();
    let mut activate = String::new();
    let mut rollback = String::new();
    let mut stale = String::new();
    let mut cleanup = String::new();
    for file in changed {
        let parent = Path::new(&file.remote)
            .parent()
            .expect("database deploy path must have a parent");
        snapshot.push_str(&format!(
            "mkdir -p {parent}; if test -e {path}; then cp -p {path} {backup}; else : > {missing}; fi; ",
            parent = sh(&parent.to_string_lossy()),
            path = sh(&file.remote),
            backup = sh(&format!("{}.rollback", file.remote)),
            missing = sh(&format!("{}.rollback-missing", file.remote)),
        ));
        verify.push_str(&format!(
            "actual=$(sha256sum {upload} | awk '{{print $1}}'); test \"$actual\" = {expected}; ",
            upload = sh(&format!("{}.upload", file.remote)),
            expected = sh(&file.sha256),
        ));
        rollback.push_str(&format!(
            "if test -e {backup}; then mv -f {backup} {path}; elif test -e {missing}; then rm -f {path} {missing}; fi; rm -f {upload}; ",
            path = sh(&file.remote),
            backup = sh(&format!("{}.rollback", file.remote)),
            missing = sh(&format!("{}.rollback-missing", file.remote)),
            upload = sh(&format!("{}.upload", file.remote)),
        ));
        stale.push_str(&format!(
            "rm -f {backup} {missing}; ",
            backup = sh(&format!("{}.rollback", file.remote)),
            missing = sh(&format!("{}.rollback-missing", file.remote)),
        ));
        cleanup.push_str(&format!(
            "rm -f {backup} {missing} {upload}; ",
            backup = sh(&format!("{}.rollback", file.remote)),
            missing = sh(&format!("{}.rollback-missing", file.remote)),
            upload = sh(&format!("{}.upload", file.remote)),
        ));
    }
    for file in changed.iter().filter(|file| {
        !file.remote.ends_with("game-databases-SHA256SUMS")
            && !file.remote.ends_with("game-databases-manifest.json")
    }) {
        activate.push_str(&format!(
            "mv -f {upload} {path}; ",
            upload = sh(&format!("{}.upload", file.remote)),
            path = sh(&file.remote),
        ));
    }
    for suffix in ["game-databases-SHA256SUMS", "game-databases-manifest.json"] {
        if let Some(file) = changed.iter().find(|file| file.remote.ends_with(suffix)) {
            activate.push_str(&format!(
                "mv -f {upload} {path}; ",
                upload = sh(&format!("{}.upload", file.remote)),
                path = sh(&file.remote),
            ));
        }
    }
    let safety = platform_safety_script();
    format!(
        "set -eu; {safety}; {stale} rollback() {{ {rollback} sync; }}; trap rollback EXIT INT TERM; {snapshot} {verify} {activate} sync; trap - EXIT INT TERM; {cleanup} sync"
    )
}

fn checked_deploy_output(label: &str, output: ExecOutput) -> Result<ExecOutput> {
    if let Some(message) = exec_failure_message(label, &output) {
        Err(message.into())
    } else {
        Ok(output)
    }
}

pub(super) fn platform_rollback_script() -> String {
    let mut rollback = String::from("set -eu; ");
    for (_, remote) in platform_deploy_files() {
        rollback.push_str(&format!(
            "if [ -e {backup} ]; then mv -f {backup} {path}; elif [ -e {missing} ]; then rm -f {path} {missing}; fi; ",
            path = sh(&remote), backup = sh(&format!("{remote}.rollback")),
            missing = sh(&format!("{remote}.rollback-missing"))
        ));
    }
    rollback.push_str(
        "mv -f /media/fat/MiSTer.ini.platform-rollback /media/fat/MiSTer.ini 2>/dev/null || true; sync",
    );
    let safety = platform_safety_script();
    shell_sequence([rollback.as_str(), safety.as_str()])
}

pub(super) fn platform_snapshot_script() -> String {
    let safety = platform_safety_script();
    let mut cleanup = String::from("rm -f /media/fat/MiSTer.ini.platform-rollback; ");
    let mut snapshot = String::new();
    for (_, remote) in platform_deploy_files() {
        let parent = Path::new(&remote)
            .parent()
            .expect("platform deploy path must have a parent");
        cleanup.push_str(&format!(
            "rm -f {backup} {missing}; ",
            backup = sh(&format!("{remote}.rollback")),
            missing = sh(&format!("{remote}.rollback-missing"))
        ));
        snapshot.push_str(&format!(
            "if [ -e {path} ]; then cp -p {path} {backup}; else mkdir -p {parent}; : > {missing}; fi; ",
            path = sh(&remote),
            backup = sh(&format!("{remote}.rollback")),
            missing = sh(&format!("{remote}.rollback-missing")),
            parent = sh(&parent.to_string_lossy())
        ));
    }
    format!(
        "set -eu; {safety}; cleanup() {{ {cleanup} }}; cleanup; trap cleanup EXIT INT TERM; cp -p /media/fat/MiSTer.ini /media/fat/MiSTer.ini.platform-rollback; {snapshot} sync; trap - EXIT INT TERM"
    )
}

pub(super) fn platform_cleanup_script() -> String {
    let mut commands = vec!["set -eu".to_string(), platform_safety_script()];
    for (_, remote) in platform_deploy_files() {
        commands.push(format!(
            "rm -f {} {}",
            sh(&format!("{remote}.rollback")),
            sh(&format!("{remote}.rollback-missing"))
        ));
    }
    commands.push("rm -f /media/fat/MiSTer.ini.platform-rollback".to_string());
    commands.push("sync".to_string());
    shell_sequence(commands.iter().map(String::as_str))
}

pub(super) fn platform_safety_script() -> String {
    let paths = arming_paths()
        .iter()
        .map(|path| sh(path))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "for path in {paths}; do if test -e \"$path\"; then printf 'platform safety blocked: %s\\n' \"$path\" >&2; exit 1; fi; done",
    )
}
