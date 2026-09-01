// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::installed_layout::{app_path, arming_paths, paths};
use super::{
    DeliveryTransferMetrics, DeployRemote, ExecOutput, Layout, Path, PathBuf, Result, Session,
    SshDeployRemote, exec_failure_message, file_sha256, fs, put_measured, sh, shell_sequence,
};

const SNES_ARTWORK_SHA256: &str =
    "7a76993e7e1b0063832b94e9d2ad588549587cf09a14ac2ced72d349ed12f766";
const SETTINGS_ARTWORK_SHA256: &str =
    "44d657ff706a49fd8c8999b7c02ea4cdb7e4a8488a54dc68e0b79235dc40e8ec";

pub(super) fn installed_platform_verify_command(layout: Layout) -> String {
    let installed = paths(layout);
    let root = installed.root;
    let main = installed.main;
    format!(
        r#"set -eu
root={root}
manifest="$root/platform-v3.manifest"
fail() {{ printf 'platform verification: %s\n' "$1" >&2; exit 1; }}
require() {{
    label="$1"
    path="$2"
    mode="$3"
    if [ "$mode" = x ] && [ ! -x "$path" ]; then fail "$label is missing or not executable: $path"; fi
    if [ "$mode" = r ] && [ ! -r "$path" ]; then fail "$label is missing or unreadable: $path"; fi
}}
check_hash() {{
    label="$1"
    path="$2"
    expected="$3"
    actual=$(sha256sum "$path" | awk '{{print $1}}')
    if [ "$actual" != "$expected" ]; then
        fail "$label hash mismatch path=$path expected=$expected actual=$actual"
    fi
}}
test -s "$manifest" || fail "manifest is missing or empty: $manifest"
require "Main" {main} x
require "GUI" "$root/mister-magik-fb" x
require "manager" "$root/mister-magik-manager" x
require "scanout module" "$root/mister_magik_scanout_slots.ko" r
require "scanout metadata" "$root/mister_magik_scanout_slots.metadata.txt" r
require "FPGA RBF" "$root/fpga/menu-magik-vblank-latch.rbf" r
require "FPGA metadata" "$root/fpga/menu-magik-vblank-latch.metadata.txt" r
require "SNES artwork" "$root/assets/snes/snes-small-v1.rgb565a" r
require "settings artwork" "$root/assets/ui/settings-v1.rgb565a" r
grep -qx 'format={manifest_format}' "$manifest" || fail "manifest format is not {manifest_format}"
get() {{
    values=$(sed -n "s/^$1=//p" "$manifest")
    [ -n "$values" ] || fail "manifest key is missing or empty: $1"
    count=$(printf '%s\n' "$values" | wc -l | tr -d ' ')
    [ "$count" -eq 1 ] || fail "manifest key is duplicated: $1"
    printf '%s' "$values"
}}
check_hash "Main" {main} "$(get main_sha256)"
check_hash "GUI" "$root/mister-magik-fb" "$(get gui_sha256)"
check_hash "manager" "$root/mister-magik-manager" "$(get manager_sha256)"
check_hash "scanout module" "$root/mister_magik_scanout_slots.ko" "$(get scanout_module_sha256)"
check_hash "scanout metadata" "$root/mister_magik_scanout_slots.metadata.txt" "$(get scanout_metadata_sha256)"
check_hash "FPGA RBF" "$root/fpga/menu-magik-vblank-latch.rbf" "$(get latch_rbf_sha256)"
check_hash "FPGA metadata" "$root/fpga/menu-magik-vblank-latch.metadata.txt" "$(get latch_metadata_sha256)"
check_hash "SNES artwork" "$root/assets/snes/snes-small-v1.rgb565a" "{SNES_ARTWORK_SHA256}"
check_hash "settings artwork" "$root/assets/ui/settings-v1.rgb565a" "{SETTINGS_ARTWORK_SHA256}""#,
        root = sh(root),
        main = sh(main),
        manifest_format = crate::platform_manifest::FORMAT,
    )
}

pub(super) fn platform_deploy_files() -> Vec<(&'static str, String)> {
    let installed = paths(Layout::Development);
    let files = vec![
        ("mister-magik-fb", installed.gui.to_owned()),
        (
            "mister-magik-agent",
            app_path(Layout::Development, "mister-magik-agent").expect("static installed path"),
        ),
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
            "assets/ui/settings-v1.rgb565a",
            app_path(Layout::Development, "assets/ui/settings-v1.rgb565a")
                .expect("static installed path"),
        ),
        (
            "platform-v3.manifest",
            app_path(Layout::Development, "platform-v3.manifest").expect("static installed path"),
        ),
    ];
    files
}

pub(super) fn database_deploy_files() -> Vec<(&'static str, String)> {
    [
        "magik-metadata-v1.bin",
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

fn legacy_database_paths() -> Vec<String> {
    ["mame.sqlite3", "hbmame.sqlite3"]
        .into_iter()
        .map(|name| app_path(Layout::Development, name).expect("static installed path"))
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
        self.run_with(&SshDeployRemote { sess, agent: None }, metrics)
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
        self.run_with(&SshDeployRemote { sess, agent: None }, metrics)
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
        let legacy_paths = legacy_database_paths();
        let legacy_inventory = remote.exec(&database_legacy_inventory_command(&legacy_paths))?;
        if let Some(message) = exec_failure_message("legacy database inventory", &legacy_inventory)
        {
            return Err(message.into());
        }
        let prune = parse_legacy_database_inventory(&legacy_paths, &legacy_inventory.stdout)?;
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
        if changed.is_empty() && prune.is_empty() {
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
        let compact = transaction
            .files
            .iter()
            .find(|file| file.remote.ends_with("/magik-metadata-v1.bin"))
            .expect("database transaction includes compact metadata");
        let output = remote.exec(&database_activation_script(&changed, &prune, compact))?;
        if let Some(message) = exec_failure_message("database activation", &output) {
            return Err(message.into());
        }
        println!(
            "database deploy ok stage={} changed_files={} skipped_files={} pruned_files={} transferred_bytes={} transfer_ms={}",
            transaction.stage.display(),
            report.changed_files,
            report.skipped_files,
            prune.len(),
            report.transferred_bytes,
            report.transfer_ms,
        );
        Ok(report)
    }
}

fn database_legacy_inventory_command(paths: &[String]) -> String {
    let mut command = String::from("set -eu; ");
    for path in paths {
        command.push_str(&format!(
            "if test -e {path}; then printf 'present  %s\\n' {path}; else printf 'missing  %s\\n' {path}; fi; ",
            path = sh(path),
        ));
    }
    command
}

fn parse_legacy_database_inventory<'a>(paths: &'a [String], stdout: &str) -> Result<Vec<&'a str>> {
    let lines = stdout.lines().collect::<Vec<_>>();
    if lines.len() != paths.len() {
        return Err(format!(
            "legacy database inventory returned {} lines for {} files",
            lines.len(),
            paths.len()
        )
        .into());
    }
    lines
        .into_iter()
        .zip(paths)
        .filter_map(|(line, expected)| {
            let mut fields = line.split_whitespace();
            let state = fields.next().unwrap_or_default();
            let path = fields.next().unwrap_or_default();
            if path != expected || fields.next().is_some() {
                return Some(Err(format!(
                    "legacy database inventory path mismatch: expected {expected} got {path}"
                )
                .into()));
            }
            match state {
                "present" => Some(Ok(expected.as_str())),
                "missing" => None,
                _ => Some(Err(format!(
                    "legacy database inventory invalid state for {expected}: {state}"
                )
                .into())),
            }
        })
        .collect()
}

fn database_activation_script(
    changed: &[&PlatformDeployFile],
    prune: &[&str],
    compact: &PlatformDeployFile,
) -> String {
    let mut snapshot = String::new();
    let mut verify = String::new();
    let mut activate = String::new();
    let mut rollback = String::new();
    let mut stale = String::new();
    let mut cleanup = String::new();
    let mut prune_files = String::new();
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
    for path in prune {
        let parent = Path::new(path)
            .parent()
            .expect("legacy database path must have a parent");
        snapshot.push_str(&format!(
            "mkdir -p {parent}; if test -e {path}; then cp -p {path} {backup}; else : > {missing}; fi; ",
            parent = sh(&parent.to_string_lossy()),
            path = sh(path),
            backup = sh(&format!("{path}.rollback")),
            missing = sh(&format!("{path}.rollback-missing")),
        ));
        rollback.push_str(&format!(
            "if test -e {backup}; then mv -f {backup} {path}; elif test -e {missing}; then rm -f {path} {missing}; fi; ",
            path = sh(path),
            backup = sh(&format!("{path}.rollback")),
            missing = sh(&format!("{path}.rollback-missing")),
        ));
        stale.push_str(&format!(
            "rm -f {backup} {missing}; ",
            backup = sh(&format!("{path}.rollback")),
            missing = sh(&format!("{path}.rollback-missing")),
        ));
        cleanup.push_str(&format!(
            "rm -f {backup} {missing}; ",
            backup = sh(&format!("{path}.rollback")),
            missing = sh(&format!("{path}.rollback-missing")),
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
    let verify_compact = format!(
        "actual=$(sha256sum {path} | awk '{{print $1}}'); test \"$actual\" = {expected}; ",
        path = sh(&compact.remote),
        expected = sh(&compact.sha256),
    );
    for path in prune {
        prune_files.push_str(&format!("rm -f {}; ", sh(path)));
    }
    let safety = platform_safety_script();
    format!(
        "set -eu; {safety}; {stale} rollback() {{ {rollback} sync; }}; trap rollback EXIT INT TERM; {snapshot} {verify} {activate} {verify_compact} {prune_files} sync; trap - EXIT INT TERM; {cleanup} sync"
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct ScriptedDatabaseRemote {
        inventory: String,
        legacy_inventory: String,
        events: RefCell<Vec<String>>,
        fail_command_containing: Option<&'static str>,
    }

    impl DeployRemote for ScriptedDatabaseRemote {
        fn exec(&self, command: &str) -> Result<ExecOutput> {
            self.events.borrow_mut().push(format!("exec {command}"));
            if self
                .fail_command_containing
                .is_some_and(|needle| command.contains(needle))
            {
                return Ok(ExecOutput {
                    rc: 9,
                    stdout: "scripted failure".into(),
                    stderr: String::new(),
                });
            }
            Ok(ExecOutput {
                rc: 0,
                stdout: if command.contains("printf 'present  %s") {
                    self.legacy_inventory.clone()
                } else if command.contains("sum=$(awk")
                    || command.contains("/media/fat/mister-magik-dev/magik-metadata-v1.bin")
                {
                    self.inventory.clone()
                } else {
                    String::new()
                },
                stderr: String::new(),
            })
        }

        fn put(&self, local: &Path, remote: &str) -> Result<()> {
            self.events
                .borrow_mut()
                .push(format!("put {} {remote}", local.display()));
            Ok(())
        }
    }

    fn database_stage(label: &str) -> (PathBuf, DatabaseDeployTransaction) {
        let stage = std::env::temp_dir().join(format!(
            "mister-magik-database-transaction-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&stage);
        fs::create_dir_all(&stage).unwrap();
        for (relative, _) in database_deploy_files() {
            let path = stage.join(relative);
            fs::write(path, relative.as_bytes()).unwrap();
        }
        let transaction = DatabaseDeployTransaction::validate(&stage).unwrap();
        (stage, transaction)
    }

    fn database_inventory(
        transaction: &DatabaseDeployTransaction,
        changed_or_missing: &[(&str, bool)],
    ) -> String {
        transaction
            .0
            .files
            .iter()
            .map(|file| {
                match changed_or_missing
                    .iter()
                    .find(|(remote, _)| *remote == file.remote)
                {
                    Some((_, true)) => format!("missing  {}", file.remote),
                    Some((_, false)) => format!("{}  {}", "0".repeat(64), file.remote),
                    None => format!("{}  {}", file.sha256, file.remote),
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn database_legacy_inventory(present: &[&str]) -> String {
        legacy_database_paths()
            .into_iter()
            .map(|path| {
                if present.contains(&path.as_str()) {
                    format!("present  {path}")
                } else {
                    format!("missing  {path}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn database_deploy_noop_skips_all_four_files() {
        let (stage, transaction) = database_stage("noop");
        let remote = ScriptedDatabaseRemote {
            inventory: database_inventory(&transaction, &[]),
            legacy_inventory: database_legacy_inventory(&[]),
            events: RefCell::new(Vec::new()),
            fail_command_containing: None,
        };
        let mut metrics = DeliveryTransferMetrics::default();

        let report = transaction.run_with(&remote, &mut metrics).unwrap();

        assert_eq!(report.changed_files, 0);
        assert_eq!(report.skipped_files, 4);
        assert_eq!(report.transferred_bytes, 0);
        assert_eq!(remote.events.borrow().len(), 2);
        assert_eq!(metrics, DeliveryTransferMetrics::default());
        fs::remove_dir_all(stage).unwrap();
    }

    #[test]
    fn database_deploy_prunes_legacy_files_even_when_compact_files_are_unchanged() {
        let (stage, transaction) = database_stage("prune-legacy");
        let legacy_mame = "/media/fat/mister-magik-dev/mame.sqlite3";
        let legacy_hbmame = "/media/fat/mister-magik-dev/hbmame.sqlite3";
        let remote = ScriptedDatabaseRemote {
            inventory: database_inventory(&transaction, &[]),
            legacy_inventory: database_legacy_inventory(&[legacy_mame, legacy_hbmame]),
            events: RefCell::new(Vec::new()),
            fail_command_containing: None,
        };
        let mut metrics = DeliveryTransferMetrics::default();

        let report = transaction.run_with(&remote, &mut metrics).unwrap();
        let events = remote.events.borrow();
        let activation = events.last().unwrap();

        assert_eq!(report.changed_files, 0);
        assert_eq!(report.skipped_files, 4);
        assert_eq!(metrics, DeliveryTransferMetrics::default());
        assert_eq!(events.len(), 3);
        assert!(!events.iter().any(|event| event.starts_with("put ")));
        assert!(activation.contains("trap rollback EXIT INT TERM"));
        assert!(activation.contains("mame.sqlite3.rollback"));
        assert!(activation.contains("hbmame.sqlite3.rollback"));
        assert!(
            activation.contains("sha256sum '/media/fat/mister-magik-dev/magik-metadata-v1.bin'")
        );
        let compact_verification = activation
            .find("sha256sum '/media/fat/mister-magik-dev/magik-metadata-v1.bin'")
            .unwrap();
        assert!(
            compact_verification
                < activation
                    .rfind("rm -f '/media/fat/mister-magik-dev/mame.sqlite3'")
                    .unwrap()
        );
        fs::remove_dir_all(stage).unwrap();
    }

    #[test]
    fn database_deploy_transfers_only_changed_files() {
        let (stage, transaction) = database_stage("changed");
        let changed = "/media/fat/mister-magik-dev/magik-metadata-v1.bin";
        let remote = ScriptedDatabaseRemote {
            inventory: database_inventory(&transaction, &[(changed, false)]),
            legacy_inventory: database_legacy_inventory(&[]),
            events: RefCell::new(Vec::new()),
            fail_command_containing: None,
        };
        let mut metrics = DeliveryTransferMetrics::default();

        let report = transaction.run_with(&remote, &mut metrics).unwrap();
        let events = remote.events.borrow();

        assert_eq!(report.changed_files, 1);
        assert_eq!(report.skipped_files, 3);
        assert_eq!(metrics.files, 1);
        assert!(
            events
                .iter()
                .any(|event| { event.ends_with(&format!("{changed}.upload")) })
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.starts_with("put "))
                .count(),
            1
        );
        fs::remove_dir_all(stage).unwrap();
    }

    #[test]
    fn database_deploy_activation_failure_keeps_rollback_trap() {
        let (stage, transaction) = database_stage("rollback");
        let remote = ScriptedDatabaseRemote {
            inventory: database_inventory(
                &transaction,
                &[("/media/fat/mister-magik-dev/magik-metadata-v1.bin", false)],
            ),
            legacy_inventory: database_legacy_inventory(&[]),
            events: RefCell::new(Vec::new()),
            fail_command_containing: Some("magik-metadata-v1.bin.upload"),
        };

        let error = transaction
            .run_with(&remote, &mut DeliveryTransferMetrics::default())
            .unwrap_err()
            .to_string();
        let events = remote.events.borrow();
        let activation = events.last().unwrap();

        assert!(error.contains("database activation"));
        assert!(activation.contains("trap rollback EXIT INT TERM"));
        assert!(activation.contains("magik-metadata-v1.bin.rollback"));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.starts_with("put "))
                .count(),
            1
        );
        fs::remove_dir_all(stage).unwrap();
    }
}
