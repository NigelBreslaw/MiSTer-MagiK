// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Local Apple Silicon Quartus setup and matched FPGA signoff.

use crate::error::{AgentError, AgentResult};
use crate::git;
use crate::process;
use crate::progress::{EventKind, Reporter};
use clap::Subcommand;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const MENU_REPOSITORY: &str = "https://github.com/MiSTer-devel/Menu_MiSTer.git";
const SIGNOFF_FORMAT: &str = "mister-magik-local-fpga-signoff-v2";
const QUARTUS_IMAGE: &str = "mister-magik-quartus17-apple:ubuntu18-amd64";
const QUARTUS_VERSION: &str = "17.0.0 Build 595";
const BUILD_DEADLINE: Duration = Duration::from_secs(3 * 60 * 60);
const SETUP_DEADLINE: Duration = Duration::from_secs(2 * 60 * 60);

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum FpgaCommand {
    /// Install or verify the pinned Quartus Apple Container runtime.
    Setup,
    /// Build stock, pre-observer, and latest-main variants, then run CI signoff.
    Signoff {
        /// Ignore a matching completed synthesis cache and rebuild all variants.
        #[arg(long)]
        rebuild: bool,
    },
}

pub fn execute(
    repository: &Path,
    command: &FpgaCommand,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    require_apple_silicon()?;
    match command {
        FpgaCommand::Setup => setup(repository, &local_root(repository), reporter),
        FpgaCommand::Signoff { rebuild } => signoff(repository, *rebuild, reporter),
    }
}

fn require_apple_silicon() -> AgentResult<()> {
    if std::env::consts::OS != "macos" || std::env::consts::ARCH != "aarch64" {
        return Err("local FPGA signoff requires Apple Silicon macOS".into());
    }
    Ok(())
}

fn setup(repository: &Path, local_root: &Path, reporter: &mut Reporter<'_>) -> AgentResult<()> {
    reporter.emit(
        EventKind::Progress,
        "fpga-setup",
        "Preparing the pinned Quartus Apple Container runtime",
        Some(5),
    )?;
    let installer = repository.join("scripts/install-quartus-lite-apple-container.sh");
    run_inherited(
        Command::new(&installer)
            .current_dir(repository)
            .env("MISTER_FPGA_LOCAL_ROOT", local_root),
        SETUP_DEADLINE,
        "Quartus Apple Container setup",
        reporter,
        "fpga-setup",
    )
}

fn signoff(repository: &Path, rebuild: bool, reporter: &mut Reporter<'_>) -> AgentResult<()> {
    let main_revision = git::value(repository, &["rev-parse", "refs/heads/main^{commit}"])?;
    let menu_revision = git_show(
        repository,
        &main_revision,
        "mister/platform/fpga/menu-vblank-latch/Menu_MiSTer.commit",
    )?;
    let baseline_revision = git_show(
        repository,
        &main_revision,
        "mister/platform/fpga/menu-vblank-latch/video-diagnostics-baseline.commit",
    )?;
    require_revision("main", &main_revision)?;
    require_revision("Menu", &menu_revision)?;
    require_revision("pre-observer", &baseline_revision)?;

    let build_date = git::value(
        repository,
        &[
            "show",
            "-s",
            "--format=%cd",
            "--date=format:%y%m%d",
            &main_revision,
        ],
    )?;
    let local_root = local_root(repository);
    let signoff_root = local_root.join("signoff");
    let marker = signoff_root.join("local-signoff-input-v2.txt");
    let source_root = local_root.join("sources/main");
    prepare_local_checkout(repository, &source_root, &main_revision)?;
    let synthesis_identity = component_identity(&source_root, "fpga-synthesis")?;
    let cache_manifest = cache_manifest(&synthesis_identity, &menu_revision, &baseline_revision);
    let cache_hit = !rebuild
        && fs::read_to_string(&marker).is_ok_and(|value| value == cache_manifest)
        && variants_complete(&signoff_root);

    if cache_hit {
        reporter.emit(
            EventKind::Progress,
            "fpga-synthesis",
            &format!("Reusing completed synthesis for main {main_revision}"),
            Some(70),
        )?;
    } else {
        setup(repository, &local_root, reporter)?;
        reporter.emit(
            EventKind::Progress,
            "fpga-synthesis",
            &format!("Building matched FPGA variants for main {main_revision}"),
            Some(10),
        )?;
        if rebuild {
            for flavour in ["stock", "pre-observer", "patched"] {
                remove_generated_dir(&signoff_root, &signoff_root.join(flavour))?;
            }
        }
        let baseline_root = local_root.join("sources/pre-observer");
        prepare_local_checkout(repository, &baseline_root, &baseline_revision)?;
        let menu_root = prepare_menu(&local_root, &menu_revision)?;
        let wrapper_root = write_quartus_wrappers(&local_root)?;

        build_variant(
            &source_root,
            &menu_root,
            &wrapper_root,
            &signoff_root,
            "stock",
            false,
            &main_revision,
            &build_date,
            reporter,
        )?;
        build_variant(
            &baseline_root,
            &menu_root,
            &wrapper_root,
            &signoff_root,
            "pre-observer",
            true,
            &main_revision,
            &build_date,
            reporter,
        )?;
        build_variant(
            &source_root,
            &menu_root,
            &wrapper_root,
            &signoff_root,
            "patched",
            true,
            &main_revision,
            &build_date,
            reporter,
        )?;
        fs::create_dir_all(&signoff_root)
            .map_err(|error| format!("cannot create {}: {error}", signoff_root.display()))?;
        fs::write(&marker, &cache_manifest)
            .map_err(|error| format!("cannot write {}: {error}", marker.display()))?;
    }

    reporter.emit(
        EventKind::Progress,
        "fpga-signoff",
        "Running the unchanged Quartus delta checker",
        Some(90),
    )?;
    run_delta_checker(&source_root, &signoff_root)
}

fn cache_manifest(synthesis_identity: &str, menu: &str, baseline: &str) -> String {
    format!(
        "format={SIGNOFF_FORMAT}\nfpga_synthesis_input_sha256={synthesis_identity}\nmenu_revision={menu}\nbaseline_revision={baseline}\nquartus_version={QUARTUS_VERSION}\nquartus_seed=1\nparallel_synthesis=off\nmenu_clock_groups=asynchronous\n"
    )
}

fn component_identity(repository: &Path, component: &str) -> AgentResult<String> {
    let output = Command::new("python3")
        .arg(repository.join("scripts/release/platform/platform-component-id.py"))
        .args(["component", component])
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot compute {component} identity: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot compute {component} identity: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let identity = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if identity.len() != 64 || !identity.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid {component} identity: {identity}").into());
    }
    Ok(identity)
}

fn local_root(repository: &Path) -> PathBuf {
    match std::env::var_os("MISTER_FPGA_LOCAL_ROOT") {
        Some(root) if Path::new(&root).is_absolute() => PathBuf::from(root),
        Some(root) => repository.join(root),
        None => repository.join("build/fpga-local-apple"),
    }
}

fn variants_complete(root: &Path) -> bool {
    ["stock", "pre-observer", "patched"].iter().all(|flavour| {
        let root = root.join(flavour);
        root.join("menu-magik-vblank-latch.rbf").is_file()
            && root.join("menu-magik-vblank-latch.metadata.txt").is_file()
            && root.join("menu-magik-vblank-latch.build.log").is_file()
            && root.join("reports").is_dir()
    })
}

fn require_revision(label: &str, revision: &str) -> AgentResult<()> {
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid {label} revision: {revision}").into());
    }
    Ok(())
}

fn git_show(repository: &Path, revision: &str, path: &str) -> AgentResult<String> {
    let object = format!("{revision}:{path}");
    git::value(repository, &["show", &object]).map_err(AgentError::from)
}

fn prepare_local_checkout(
    repository: &Path,
    destination: &Path,
    revision: &str,
) -> AgentResult<()> {
    if !destination.join(".git").is_dir() {
        if destination.exists() {
            remove_generated_dir(
                destination.parent().ok_or("checkout has no parent")?,
                destination,
            )?;
        }
        fs::create_dir_all(destination.parent().ok_or("checkout has no parent")?)
            .map_err(|error| format!("cannot create checkout parent: {error}"))?;
        run_status(
            Command::new("git")
                .args(["clone", "--shared", "--no-checkout"])
                .arg(repository)
                .arg(destination),
            "create isolated FPGA source checkout",
        )?;
    }
    run_status(
        Command::new("git")
            .args(["fetch", "--quiet"])
            .arg(repository)
            .arg(revision)
            .current_dir(destination),
        "refresh isolated FPGA source checkout",
    )?;
    run_status(
        Command::new("git")
            .args(["switch", "--detach", "--force", revision])
            .current_dir(destination),
        "select isolated FPGA source revision",
    )
}

fn prepare_menu(local_root: &Path, revision: &str) -> AgentResult<PathBuf> {
    let menu_root = local_root.join("sources/menu");
    let mirror = menu_root.join("Menu_MiSTer.git");
    let work = menu_root.join("work");
    fs::create_dir_all(&menu_root)
        .map_err(|error| format!("cannot create {}: {error}", menu_root.display()))?;
    if !mirror.is_dir() {
        run_status(
            Command::new("git")
                .args(["clone", "--mirror", MENU_REPOSITORY])
                .arg(&mirror),
            "clone pinned Menu repository",
        )?;
    }
    run_status(
        Command::new("git")
            .args(["fetch", "--quiet", "origin"])
            .current_dir(&mirror),
        "refresh pinned Menu repository",
    )?;
    remove_generated_dir(&menu_root, &work)?;
    run_status(
        Command::new("git")
            .args(["clone", "--shared", "--no-checkout"])
            .arg(&mirror)
            .arg(&work),
        "create disposable Menu checkout",
    )?;
    run_status(
        Command::new("git")
            .args(["switch", "--detach", revision])
            .current_dir(&work),
        "select pinned Menu revision",
    )?;
    replace_once(
        &work.join("sys/sys_top.sdc"),
        "set_clock_groups -exclusive",
        "set_clock_groups -asynchronous",
    )?;
    let qsf = work.join("menu.qsf");
    let mut source = fs::read_to_string(&qsf)
        .map_err(|error| format!("cannot read {}: {error}", qsf.display()))?;
    source.push_str(
        "\n# Apple Rosetta compatibility: Quartus synthesis helpers deadlock.\nset_global_assignment -name PARALLEL_SYNTHESIS OFF\nset_global_assignment -name AUTO_PARALLEL_SYNTHESIS OFF\n",
    );
    fs::write(&qsf, source).map_err(|error| format!("cannot write {}: {error}", qsf.display()))?;
    Ok(work)
}

fn replace_once(path: &Path, before: &str, after: &str) -> AgentResult<()> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if source.matches(before).count() != 1 {
        return Err(format!("expected exactly one {before:?} in {}", path.display()).into());
    }
    fs::write(path, source.replacen(before, after, 1))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(())
}

fn write_quartus_wrappers(local_root: &Path) -> AgentResult<PathBuf> {
    let root = local_root.join("quartus-bin");
    fs::create_dir_all(&root)
        .map_err(|error| format!("cannot create {}: {error}", root.display()))?;
    let install = local_root.join("quartus-lite-17.0/apple-intelFPGA_lite");
    let shell = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\ntool=\"${{QUARTUS_APPLE_TOOL:-$(basename \"$0\")}}\"\nwork_dir=\"$(pwd -P)\"\nexec container run --arch amd64 --rm --cpus 8 --memory 12G --mount \"type=bind,source={},target=/opt/intelFPGA_lite,readonly\" --mount \"type=bind,source=${{work_dir}},target=/work\" --workdir /work {QUARTUS_IMAGE} \"$tool\" \"$@\"\n",
        install.display()
    );
    write_executable(&root.join("quartus_sh"), &shell)?;
    write_executable(
        &root.join("quartus_sta"),
        "#!/usr/bin/env bash\nset -euo pipefail\nQUARTUS_APPLE_TOOL=quartus_sta exec \"$(dirname \"$0\")/quartus_sh\" \"$@\"\n",
    )?;
    Ok(root)
}

fn write_executable(path: &Path, source: &str) -> AgentResult<()> {
    fs::write(path, source).map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("cannot chmod {}: {error}", path.display()))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_variant(
    source_root: &Path,
    menu_root: &Path,
    wrapper_root: &Path,
    signoff_root: &Path,
    flavour: &str,
    apply_patch: bool,
    main_revision: &str,
    build_date: &str,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    let output = signoff_root.join(flavour);
    let path = prefixed_path(wrapper_root)?;
    reporter.emit(
        EventKind::Progress,
        "fpga-synthesis",
        &format!("Building {flavour} FPGA variant"),
        None,
    )?;
    let mut command = Command::new(source_root.join("scripts/build-fpga-vblank-latch-core.sh"));
    command
        .current_dir(source_root)
        .env("GITHUB_ACTIONS", "true")
        .env("PATH", path)
        .env("MISTER_MENU_DIR", menu_root)
        .env(
            "MISTER_FPGA_APPLY_PATCH",
            if apply_patch { "1" } else { "0" },
        )
        .env("MISTER_FPGA_QUARTUS_SEED", "1")
        .env("MISTER_FPGA_BUILD_DATE", build_date)
        .env("MISTER_FPGA_OUT_DIR", &output)
        .env("MISTER_MENU_BUILD_DIR", output.join("Menu-work"))
        .env("MISTER_FPGA_QUALIFIED_MAGIK_REVISION", main_revision);
    run_inherited(
        &mut command,
        BUILD_DEADLINE,
        &format!("{flavour} Quartus build"),
        reporter,
        "fpga-synthesis",
    )
}

fn prefixed_path(prefix: &Path) -> AgentResult<OsString> {
    let existing = std::env::var_os("PATH").ok_or("PATH is not set")?;
    let mut paths = vec![prefix.to_path_buf()];
    paths.extend(std::env::split_paths(&existing));
    std::env::join_paths(paths).map_err(|error| format!("cannot construct PATH: {error}").into())
}

fn run_delta_checker(source_root: &Path, signoff_root: &Path) -> AgentResult<()> {
    let mut command = Command::new(source_root.join("scripts/checks/check-fpga-quartus-delta.py"));
    for (flag, flavour) in [
        ("--stock", "stock"),
        ("--baseline", "pre-observer"),
        ("--patched", "patched"),
    ] {
        for report in reports(&signoff_root.join(flavour))? {
            command.arg(flag).arg(report);
        }
    }
    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("cannot start Quartus delta checker: {error}"))?;
    let report = signoff_root.join("quartus-delta-signoff.tsv");
    fs::write(&report, &output.stdout)
        .map_err(|error| format!("cannot write {}: {error}", report.display()))?;
    std::io::stdout()
        .write_all(&output.stdout)
        .map_err(|error| format!("cannot print Quartus delta report: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Quartus delta signoff failed with {}; retained {}",
            output.status,
            report.display()
        )
        .into());
    }
    Ok(())
}

fn reports(root: &Path) -> AgentResult<Vec<PathBuf>> {
    let mut reports = Vec::new();
    for directory in [root.to_path_buf(), root.join("reports")] {
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        for entry in entries {
            let path = entry
                .map_err(|error| format!("cannot read {} entry: {error}", directory.display()))?
                .path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(OsStr::to_str)
                    .is_some_and(|extension| matches!(extension, "log" | "rpt" | "summary"))
            {
                reports.push(path);
            }
        }
    }
    reports.sort();
    Ok(reports)
}

fn run_inherited(
    command: &mut Command,
    deadline: Duration,
    label: &str,
    reporter: &mut Reporter<'_>,
    phase: &str,
) -> AgentResult<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("cannot start {label}: {error}"))?;
    let status = process::wait(
        &mut child,
        Some(deadline),
        label,
        Some(Duration::from_secs(30)),
        || {
            reporter.emit(
                EventKind::Progress,
                phase,
                &format!("{label} is still running"),
                None,
            )
        },
    )?;
    if !status.success() {
        return Err(format!("{label} exited with {status}").into());
    }
    Ok(())
}

fn run_status(command: &mut Command, label: &str) -> AgentResult<()> {
    let status = command
        .stdin(Stdio::null())
        .status()
        .map_err(|error| format!("cannot {label}: {error}"))?;
    if !status.success() {
        return Err(format!("cannot {label}: exited with {status}").into());
    }
    Ok(())
}

fn remove_generated_dir(root: &Path, target: &Path) -> AgentResult<()> {
    if !target.exists() {
        return Ok(());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("cannot resolve generated root {}: {error}", root.display()))?;
    let parent = target.parent().ok_or("generated target has no parent")?;
    let parent = parent.canonicalize().map_err(|error| {
        format!(
            "cannot resolve generated parent {}: {error}",
            parent.display()
        )
    })?;
    if parent != root && !parent.starts_with(&root) {
        return Err(format!(
            "refusing to remove generated path outside {}",
            root.display()
        )
        .into());
    }
    fs::remove_dir_all(target)
        .map_err(|error| format!("cannot remove generated {}: {error}", target.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_manifest_binds_every_matched_build_input() {
        let manifest = cache_manifest(&"a".repeat(64), &"b".repeat(40), &"c".repeat(40));
        assert!(manifest.contains(&format!("format={SIGNOFF_FORMAT}")));
        assert!(manifest.contains("fpga_synthesis_input_sha256=aaaaaaaa"));
        assert!(manifest.contains("menu_revision=bbbbbbbb"));
        assert!(manifest.contains("baseline_revision=cccccccc"));
        assert!(manifest.contains("quartus_seed=1"));
        assert!(manifest.contains("parallel_synthesis=off"));
        assert!(manifest.contains("menu_clock_groups=asynchronous"));
    }
}
