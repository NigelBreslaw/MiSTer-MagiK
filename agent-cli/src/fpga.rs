// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Local Apple Silicon Quartus setup and matched FPGA signoff.

use crate::error::{AgentError, AgentResult};
use crate::git;
use crate::process;
use crate::progress::{EventKind, Reporter};
use clap::Subcommand;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const MENU_REPOSITORY: &str = "https://github.com/MiSTer-devel/Menu_MiSTer.git";
const SIGNOFF_FORMAT: &str = "mister-magik-local-fpga-signoff-v5";
const VARIANT_CACHE_MARKER: &str = "local-signoff-input-v5.txt";
const LEGACY_SIGNOFF_FORMAT: &str = "mister-magik-local-fpga-signoff-v4";
const LEGACY_VARIANT_CACHE_MARKER: &str = "local-signoff-input-v4.txt";
const QUARTUS_IMAGE: &str = "mister-magik-quartus17-apple:ubuntu18-amd64";
const QUARTUS_VERSION: &str = "17.0.0 Build 595";
const QUARTUS_PROCESSORS: &str = "4";
const QUARTUS_SEED_SOURCE: &str =
    include_str!("../../mister/platform/fpga/menu-vblank-latch/Quartus.seed");
const BUILD_EPOCH_SOURCE: &str = include_str!("../../scripts/quartus/fpga-build-epoch-v1.txt");
const LOCAL_SIGNOFF_PROFILE_PATH: &str =
    "mister/platform/fpga/menu-vblank-latch/local-signoff-profile.txt";
const BUILD_DEADLINE: Duration = Duration::from_secs(3 * 60 * 60);
const SETUP_DEADLINE: Duration = Duration::from_secs(2 * 60 * 60);

struct CachePolicy<'a> {
    seed: &'a str,
    build_date: &'a str,
    preparation_identity: &'a str,
}

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
    let quartus_seed = canonical_quartus_seed()?;
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

    let local_root = local_root(repository);
    let signoff_root = local_root.join("signoff");
    let source_root = local_root.join("sources/main");
    prepare_local_checkout(repository, &source_root, &main_revision)?;
    let synthesis_component = platform_component_value(&source_root, false)?;
    let build_date = canonical_build_epoch()?;
    let baseline_root = local_root.join("sources/pre-observer");
    prepare_local_checkout(repository, &baseline_root, &baseline_revision)?;
    let stock_identity = synthesis_files_identity(&source_root, false, false)?;
    let baseline_identity = synthesis_files_identity(&baseline_root, true, false)?;
    let patched_identity = synthesis_files_identity(&source_root, true, true)?;
    let preparation_identity = git::value(
        &source_root,
        &["rev-parse", "HEAD:scripts/prepare-fpga-menu-signoff.py"],
    )?;
    let cache_policy = CachePolicy {
        seed: quartus_seed,
        build_date,
        preparation_identity: &preparation_identity,
    };
    let stock_manifest = cache_manifest(
        "stock",
        &stock_identity,
        &menu_revision,
        None,
        &cache_policy,
    );
    let baseline_manifest = cache_manifest(
        "pre-observer",
        &baseline_identity,
        &menu_revision,
        Some(&baseline_revision),
        &cache_policy,
    );
    let patched_manifest = cache_manifest(
        "patched",
        &patched_identity,
        &menu_revision,
        None,
        &cache_policy,
    );

    let stock_hit = !rebuild && variant_cache_hit(&signoff_root.join("stock"), &stock_manifest);
    let baseline_hit =
        !rebuild && variant_cache_hit(&signoff_root.join("pre-observer"), &baseline_manifest);
    let patched_hit =
        !rebuild && variant_cache_hit(&signoff_root.join("patched"), &patched_manifest);

    if stock_hit && baseline_hit && patched_hit {
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
        let menu_root = prepare_menu(
            &local_root,
            &menu_revision,
            &source_root.join("scripts/prepare-fpga-menu-signoff.py"),
        )?;
        let wrapper_root = write_quartus_wrappers(&local_root)?;

        build_cached_variant(
            &source_root,
            &menu_root,
            &wrapper_root,
            &signoff_root,
            "stock",
            false,
            &main_revision,
            build_date,
            &synthesis_component,
            &stock_manifest,
            quartus_seed,
            stock_hit,
            reporter,
        )?;
        build_cached_variant(
            &baseline_root,
            &menu_root,
            &wrapper_root,
            &signoff_root,
            "pre-observer",
            true,
            &main_revision,
            build_date,
            &synthesis_component,
            &baseline_manifest,
            quartus_seed,
            baseline_hit,
            reporter,
        )?;
        build_cached_variant(
            &source_root,
            &menu_root,
            &wrapper_root,
            &signoff_root,
            "patched",
            true,
            &main_revision,
            build_date,
            &synthesis_component,
            &patched_manifest,
            quartus_seed,
            patched_hit,
            reporter,
        )?;
    }

    reporter.emit(
        EventKind::Progress,
        "fpga-signoff",
        "Running the unchanged Quartus delta checker",
        Some(90),
    )?;
    run_delta_checker(&source_root, &signoff_root)
}

fn cache_manifest(
    flavour: &str,
    synthesis_identity: &str,
    menu: &str,
    baseline: Option<&str>,
    policy: &CachePolicy<'_>,
) -> String {
    let baseline = baseline.unwrap_or("-");
    let CachePolicy {
        seed,
        build_date,
        preparation_identity,
    } = policy;
    format!(
        "format={SIGNOFF_FORMAT}\nflavour={flavour}\nsynthesis_input={synthesis_identity}\nmenu_revision={menu}\nbaseline_revision={baseline}\nquartus_version={QUARTUS_VERSION}\nquartus_seed={seed}\nbuild_date={build_date}\nquartus_processors={QUARTUS_PROCESSORS}\nparallel_synthesis=off\nauto_parallel_synthesis=off\nmenu_clock_groups=asynchronous\npreparation_input={preparation_identity}\n"
    )
}

fn canonical_build_epoch() -> AgentResult<&'static str> {
    let Some(epoch) = BUILD_EPOCH_SOURCE.strip_suffix('\n') else {
        return Err("canonical FPGA build epoch must end with one newline".into());
    };
    if epoch.len() != 6 || !epoch.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("canonical FPGA build epoch must be a six-digit YYMMDD value".into());
    }
    Ok(epoch)
}

fn canonical_quartus_seed() -> AgentResult<&'static str> {
    let Some(seed) = QUARTUS_SEED_SOURCE.strip_suffix('\n') else {
        return Err("canonical Quartus seed file must end with one newline".into());
    };
    validate_quartus_seed(seed)
}

fn validate_quartus_seed(seed: &str) -> AgentResult<&str> {
    if seed.is_empty() || seed.starts_with('0') || !seed.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("canonical Quartus seed must be a positive decimal integer".into());
    }
    Ok(seed)
}

fn synthesis_files_identity(
    repository: &Path,
    apply_patch: bool,
    include_diagnostics: bool,
) -> AgentResult<String> {
    let mut paths = vec![
        "scripts/build-fpga-vblank-latch-core.sh",
        "mister/platform/fpga/menu-vblank-latch/report_top_timing.tcl",
        "mister/platform/kernel/scanout-slots/mister_magik_scanout_platform.h",
    ];
    if apply_patch {
        paths.extend([
            "mister/platform/fpga/menu-vblank-latch/Menu_MiSTer-vblank-latched-fbuf.patch",
            "mister/platform/fpga/menu-vblank-latch/mister_magik_vblank_latch.sv",
            "mister/platform/fpga/menu-vblank-latch/mister_magik_latch_sys_top_bridge.sv",
            "mister/platform/fpga/menu-vblank-latch/mister_magik_bootstrap_black.sv",
            "mister/platform/fpga/menu-vblank-latch/mister_magik_latch_protocol.svh",
        ]);
    }
    if include_diagnostics {
        paths.extend([
            LOCAL_SIGNOFF_PROFILE_PATH,
            "mister/platform/fpga/menu-vblank-latch/mister_magik_video_diagnostics_control.sv",
            "mister/platform/fpga/menu-vblank-latch/mister_magik_video_diagnostics_avalon.sv",
            "mister/platform/fpga/menu-vblank-latch/mister_magik_video_diagnostics_output.sv",
            "mister/platform/fpga/menu-vblank-latch/mister_magik_video_diagnostics_protocol.svh",
            "mister/platform/fpga/menu-vblank-latch/mister_magik_video_diagnostics.sdc",
        ]);
    }
    let mut blobs = Vec::new();
    for path in paths {
        let object = format!("HEAD:{path}");
        let blob = git::value(repository, &["rev-parse", &object])?;
        if blob.len() != 40 || !blob.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("invalid synthesis blob for {path}: {blob}").into());
        }
        blobs.push(blob);
    }
    Ok(blobs.join(":"))
}

fn local_root(repository: &Path) -> PathBuf {
    match std::env::var_os("MISTER_FPGA_LOCAL_ROOT") {
        Some(root) if Path::new(&root).is_absolute() => PathBuf::from(root),
        Some(root) => repository.join(root),
        None => repository.join("build/fpga-local-apple"),
    }
}

fn variant_complete(root: &Path) -> bool {
    root.join("menu-magik-vblank-latch.rbf").is_file()
        && root.join("menu-magik-vblank-latch.metadata.txt").is_file()
        && root.join("menu-magik-vblank-latch.build.log").is_file()
        && root.join("reports").is_dir()
}

fn variant_cache_hit(root: &Path, expected_manifest: &str) -> bool {
    if !variant_complete(root) || !variant_artifacts_match_metadata(root) {
        return false;
    }
    if fs::read_to_string(root.join(VARIANT_CACHE_MARKER))
        .is_ok_and(|value| value == expected_manifest)
    {
        return true;
    }
    let Ok(legacy) = fs::read_to_string(root.join(LEGACY_VARIANT_CACHE_MARKER)) else {
        return false;
    };
    if !legacy_cache_manifest_matches(&legacy, expected_manifest) {
        return false;
    }
    let temporary = root.join(format!(".{VARIANT_CACHE_MARKER}.tmp"));
    fs::write(&temporary, expected_manifest)
        .and_then(|()| fs::rename(&temporary, root.join(VARIANT_CACHE_MARKER)))
        .is_ok()
}

fn parse_cache_manifest(source: &str) -> Option<BTreeMap<&str, &str>> {
    let mut fields = BTreeMap::new();
    for line in source.lines() {
        let (key, value) = line.split_once('=')?;
        if key.is_empty() || value.is_empty() || fields.insert(key, value).is_some() {
            return None;
        }
    }
    Some(fields)
}

fn legacy_cache_manifest_matches(legacy: &str, expected: &str) -> bool {
    let (Some(mut legacy), Some(mut expected)) =
        (parse_cache_manifest(legacy), parse_cache_manifest(expected))
    else {
        return false;
    };
    if legacy.remove("format") != Some(LEGACY_SIGNOFF_FORMAT)
        || expected.remove("format") != Some(SIGNOFF_FORMAT)
    {
        return false;
    }
    let Some(component) = legacy.remove("synthesis_component") else {
        return false;
    };
    component.len() == 64
        && component.bytes().all(|byte| byte.is_ascii_hexdigit())
        && legacy == expected
}

fn variant_artifacts_match_metadata(root: &Path) -> bool {
    let Ok(metadata) = fs::read_to_string(root.join("menu-magik-vblank-latch.metadata.txt")) else {
        return false;
    };
    let mut fields = BTreeMap::new();
    for line in metadata.lines() {
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        if key == "source_status" {
            continue;
        }
        if key.is_empty() || value.is_empty() || fields.insert(key, value).is_some() {
            return false;
        }
    }
    if fields.get("build_date").copied() != canonical_build_epoch().ok()
        || fields.get("quartus_seed").copied() != canonical_quartus_seed().ok()
        || fields.get("quartus_version").copied() != Some(QUARTUS_VERSION)
        || fields.get("quartus_processors").copied() != Some(QUARTUS_PROCESSORS)
        || fields.get("parallel_synthesis").copied() != Some("off")
        || fields.get("auto_parallel_synthesis").copied() != Some("off")
    {
        return false;
    }
    let rbf_name = fields
        .get("rbf_file")
        .copied()
        .unwrap_or("menu-magik-vblank-latch.rbf");
    if Path::new(rbf_name).components().count() != 1
        || fields.get("rbf_sha256").copied() != digest_file(&root.join(rbf_name)).as_deref()
    {
        return false;
    }
    let reports: Vec<_> = fields
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix("report_sha256.")
                .map(|path| (path, *value))
        })
        .collect();
    !reports.is_empty()
        && reports.into_iter().all(|(relative, expected)| {
            relative.starts_with("reports/")
                && !relative
                    .split('/')
                    .any(|part| part.is_empty() || part == "..")
                && digest_file(&root.join(relative)).as_deref() == Some(expected)
        })
}

fn digest_file(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    Some(encoded)
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

fn platform_component_value(repository: &Path, revision_only: bool) -> AgentResult<String> {
    let script = repository.join("scripts/release/platform/platform-component-id.py");
    let mut command = Command::new("python3");
    command
        .arg(script)
        .args(["component", "fpga-synthesis"])
        .current_dir(repository);
    if revision_only {
        command.arg("--revision-only");
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot compute FPGA synthesis identity: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot compute FPGA synthesis identity: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|error| format!("invalid FPGA synthesis identity output: {error}"))?;
    let value = value.trim();
    let expected_length = if revision_only { 40 } else { 64 };
    if value.len() != expected_length || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid FPGA synthesis identity: {value}").into());
    }
    Ok(value.to_owned())
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

fn prepare_menu(
    local_root: &Path,
    revision: &str,
    preparation_script: &Path,
) -> AgentResult<PathBuf> {
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
    run_status(
        Command::new("python3").arg(preparation_script).arg(&work),
        "apply canonical Menu signoff settings",
    )?;
    Ok(work)
}

fn write_quartus_wrappers(local_root: &Path) -> AgentResult<PathBuf> {
    let root = local_root.join("quartus-bin");
    fs::create_dir_all(&root)
        .map_err(|error| format!("cannot create {}: {error}", root.display()))?;
    let install = local_root.join("quartus-lite-17.0/apple-intelFPGA_lite");
    let shell = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\ntool=\"${{QUARTUS_APPLE_TOOL:-$(basename \"$0\")}}\"\nwork_dir=\"$(pwd -P)\"\nexec container run --arch amd64 --rm --cpus {QUARTUS_PROCESSORS} --memory 12G --mount \"type=bind,source={},target=/opt/intelFPGA_lite,readonly\" --mount \"type=bind,source=${{work_dir}},target=/work\" --workdir /work {QUARTUS_IMAGE} \"$tool\" \"$@\"\n",
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
fn build_cached_variant(
    source_root: &Path,
    menu_root: &Path,
    wrapper_root: &Path,
    signoff_root: &Path,
    flavour: &str,
    apply_patch: bool,
    main_revision: &str,
    build_date: &str,
    synthesis_component: &str,
    cache_manifest: &str,
    quartus_seed: &str,
    cache_hit: bool,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    if cache_hit {
        reporter.emit(
            EventKind::Progress,
            "fpga-synthesis",
            &format!("Reusing completed {flavour} FPGA variant"),
            None,
        )?;
        return Ok(());
    }
    fs::create_dir_all(signoff_root)
        .map_err(|error| format!("cannot create {}: {error}", signoff_root.display()))?;
    let output = signoff_root.join(flavour);
    let staging = signoff_root.join(format!(".{flavour}.building"));
    remove_generated_dir(signoff_root, &staging)?;
    build_variant(
        source_root,
        menu_root,
        wrapper_root,
        &staging,
        flavour,
        apply_patch,
        main_revision,
        build_date,
        synthesis_component,
        quartus_seed,
        reporter,
    )?;
    append_variant_policy(&staging)?;
    fs::write(staging.join(VARIANT_CACHE_MARKER), cache_manifest).map_err(|error| {
        format!(
            "cannot write staged {flavour} cache marker {}: {error}",
            staging.join(VARIANT_CACHE_MARKER).display()
        )
    })?;
    promote_variant(signoff_root, flavour, &staging, &output)
}

fn append_variant_policy(output: &Path) -> AgentResult<()> {
    let path = output.join("menu-magik-vblank-latch.metadata.txt");
    let mut metadata = OpenOptions::new()
        .append(true)
        .open(&path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    write!(
        metadata,
        "quartus_processors={QUARTUS_PROCESSORS}\nparallel_synthesis=off\nauto_parallel_synthesis=off\n"
    )
    .map_err(|error| format!("cannot append {}: {error}", path.display()).into())
}

#[allow(clippy::too_many_arguments)]
fn build_variant(
    source_root: &Path,
    menu_root: &Path,
    wrapper_root: &Path,
    output: &Path,
    flavour: &str,
    apply_patch: bool,
    main_revision: &str,
    build_date: &str,
    synthesis_component: &str,
    quartus_seed: &str,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
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
        .env("MISTER_FPGA_QUARTUS_SEED", quartus_seed)
        .env("QUARTUS_DOCKER_CPUS", QUARTUS_PROCESSORS)
        .env("MISTER_FPGA_SYNTHESIS_INPUT_SHA256", synthesis_component)
        .env("MISTER_FPGA_BUILD_DATE", build_date)
        .env("MISTER_FPGA_OUT_DIR", output)
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

fn promote_variant(
    signoff_root: &Path,
    flavour: &str,
    staging: &Path,
    output: &Path,
) -> AgentResult<()> {
    let backup = signoff_root.join(format!(".{flavour}.previous"));
    remove_generated_dir(signoff_root, &backup)?;
    if output.exists() {
        fs::rename(output, &backup).map_err(|error| {
            format!(
                "cannot preserve previous {flavour} cache {}: {error}",
                output.display()
            )
        })?;
    }
    if let Err(error) = fs::rename(staging, output) {
        if backup.exists() {
            let _ = fs::rename(&backup, output);
        }
        return Err(format!(
            "cannot promote completed {flavour} cache {}: {error}",
            staging.display()
        )
        .into());
    }
    remove_generated_dir(signoff_root, &backup)
}

fn prefixed_path(prefix: &Path) -> AgentResult<OsString> {
    let existing = std::env::var_os("PATH").ok_or("PATH is not set")?;
    let mut paths = vec![prefix.to_path_buf()];
    paths.extend(std::env::split_paths(&existing));
    std::env::join_paths(paths).map_err(|error| format!("cannot construct PATH: {error}").into())
}

fn run_delta_checker(source_root: &Path, signoff_root: &Path) -> AgentResult<()> {
    let mut command = Command::new(source_root.join("scripts/checks/check-fpga-quartus-delta.py"));
    let profile = fs::read_to_string(source_root.join(LOCAL_SIGNOFF_PROFILE_PATH))
        .map_err(|error| format!("cannot read checked-in FPGA local signoff profile: {error}"))?;
    match validate_local_signoff_profile(&profile)? {
        LocalSignoffProfile::Production => {}
        LocalSignoffProfile::RawScaler => {
            command.arg("--experimental-diagnostic");
        }
        LocalSignoffProfile::ScalerFetch => {
            command.arg("--experimental-scaler-fetch");
        }
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalSignoffProfile {
    Production,
    RawScaler,
    ScalerFetch,
}

fn validate_local_signoff_profile(profile: &str) -> AgentResult<LocalSignoffProfile> {
    match profile.trim() {
        "production-v1" => Ok(LocalSignoffProfile::Production),
        "experimental_raw_scaler-v1" => Ok(LocalSignoffProfile::RawScaler),
        "experimental_scaler_fetch-v1" => Ok(LocalSignoffProfile::ScalerFetch),
        value => Err(format!("unsupported checked-in FPGA local signoff profile {value:?}").into()),
    }
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
        let seed = canonical_quartus_seed().unwrap();
        let policy = CachePolicy {
            seed,
            build_date: "260813",
            preparation_identity: &"d".repeat(40),
        };
        let manifest = cache_manifest(
            "patched",
            &"a".repeat(64),
            &"b".repeat(40),
            Some(&"c".repeat(40)),
            &policy,
        );
        assert!(manifest.contains(&format!("format={SIGNOFF_FORMAT}")));
        assert!(manifest.contains("flavour=patched"));
        assert!(manifest.contains("synthesis_input=aaaaaaaa"));
        assert!(!manifest.contains("synthesis_component="));
        assert!(manifest.contains("menu_revision=bbbbbbbb"));
        assert!(manifest.contains("baseline_revision=cccccccc"));
        assert_eq!(seed, "2");
        assert!(manifest.contains("quartus_seed=2"));
        assert!(manifest.contains("build_date=260813"));
        assert!(manifest.contains("quartus_processors=4"));
        assert!(manifest.contains("parallel_synthesis=off"));
        assert!(manifest.contains("auto_parallel_synthesis=off"));
        assert!(manifest.contains("menu_clock_groups=asynchronous"));
        assert!(manifest.contains("preparation_input=dddddddd"));
    }

    #[test]
    fn canonical_seed_rejects_malformed_values() {
        for seed in ["", "0", "01", "-1", "2 3", "2\n3", "seed"] {
            assert!(validate_quartus_seed(seed).is_err(), "accepted {seed:?}");
        }
        assert_eq!(validate_quartus_seed("2").unwrap(), "2");
    }

    #[test]
    fn cache_manifest_is_independent_per_variant() {
        let seed = canonical_quartus_seed().unwrap();
        let policy = CachePolicy {
            seed,
            build_date: "date",
            preparation_identity: "policy",
        };
        let stock = cache_manifest("stock", "stock-input", "menu", None, &policy);
        let baseline = cache_manifest(
            "pre-observer",
            "baseline-input",
            "menu",
            Some("baseline"),
            &policy,
        );
        let patched = cache_manifest("patched", "patched-input", "menu", None, &policy);
        assert_ne!(stock, baseline);
        assert_ne!(baseline, patched);
        assert_ne!(stock, patched);
    }

    #[test]
    fn canonical_build_epoch_is_stable_and_well_formed() {
        assert_eq!(canonical_build_epoch().unwrap(), "260814");
    }

    #[test]
    fn checked_in_local_signoff_profile_is_fail_closed() {
        assert_eq!(
            validate_local_signoff_profile("production-v1\n").unwrap(),
            LocalSignoffProfile::Production
        );
        assert_eq!(
            validate_local_signoff_profile("experimental_raw_scaler-v1\n").unwrap(),
            LocalSignoffProfile::RawScaler
        );
        assert_eq!(
            validate_local_signoff_profile("experimental_scaler_fetch-v1\n").unwrap(),
            LocalSignoffProfile::ScalerFetch
        );
        assert!(validate_local_signoff_profile("experimental\n").is_err());
        assert!(validate_local_signoff_profile("").is_err());
    }

    #[test]
    fn legacy_cache_manifest_migrates_without_global_component_identity() {
        let policy = CachePolicy {
            seed: "2",
            build_date: "260814",
            preparation_identity: "preparation",
        };
        let expected = cache_manifest("stock", "stock-input", "menu", None, &policy);
        let legacy = expected
            .replace(SIGNOFF_FORMAT, LEGACY_SIGNOFF_FORMAT)
            .replace(
                "menu_revision=",
                &format!("synthesis_component={}\nmenu_revision=", "e".repeat(64)),
            );
        assert!(legacy_cache_manifest_matches(&legacy, &expected));
        assert!(!legacy_cache_manifest_matches(
            &legacy.replace("build_date=260814", "build_date=260813"),
            &expected,
        ));
        assert!(!legacy_cache_manifest_matches(
            &legacy.replace(&"e".repeat(64), "not-a-hash"),
            &expected,
        ));
    }
}
