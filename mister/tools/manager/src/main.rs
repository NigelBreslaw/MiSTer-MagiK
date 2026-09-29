// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_ini::Document;
use mister_magik_platform_manifest_contract::{
    Layout as ManifestLayout, ParsedManifest, ValidationProfile,
};
use mister_magik_scanout_contract::{
    DEVELOPMENT_KERNEL_REVISION, DEVELOPMENT_PROFILE, resolve_profile,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputEvent {
    Up,
    Down,
    Confirm,
    Cancel,
    Other,
}

#[derive(Default)]
struct InputDecoder {
    bytes: Vec<u8>,
}

impl InputDecoder {
    fn push(&mut self, bytes: &[u8]) -> Option<InputEvent> {
        self.bytes.extend_from_slice(bytes);
        match self.bytes.as_slice() {
            [b'\n' | b'\r', ..] => Some(InputEvent::Confirm),
            [0x1b, b'[' | b'O', b'A', ..] => Some(InputEvent::Up),
            [0x1b, b'[' | b'O', b'B', ..] => Some(InputEvent::Down),
            [0x1b] | [0x1b, b'[' | b'O'] => None,
            [0x1b, ..] => Some(InputEvent::Cancel),
            [] => None,
            _ => Some(InputEvent::Other),
        }
    }

    #[cfg(test)]
    fn finish(&self) -> Option<InputEvent> {
        match self.bytes.as_slice() {
            [] => None,
            [0x1b] | [0x1b, b'[' | b'O'] => Some(InputEvent::Cancel),
            _ => None,
        }
    }
}

struct TerminalMode {
    fd: RawFd,
    original: libc::termios,
    active: bool,
}

impl TerminalMode {
    fn enter(fd: RawFd) -> io::Result<Self> {
        let original = terminal_settings(fd)
            .map_err(|error| terminal_error("read terminal settings", error))?;
        let mut raw = original;
        raw.c_lflag &= !(libc::ECHO | libc::ICANON);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        set_terminal_settings(fd, &raw)
            .map_err(|error| terminal_error("enable terminal key input", error))?;
        Ok(Self {
            fd,
            original,
            active: true,
        })
    }

    #[cfg(test)]
    fn set_tail_timeout(&mut self) -> io::Result<()> {
        let mut timed = self.original;
        timed.c_lflag &= !(libc::ECHO | libc::ICANON);
        timed.c_cc[libc::VMIN] = 0;
        timed.c_cc[libc::VTIME] = 1;
        set_terminal_settings(self.fd, &timed)
            .map_err(|error| terminal_error("configure terminal key timeout", error))
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        set_terminal_settings(self.fd, &self.original)
            .map_err(|error| terminal_error("restore terminal settings", error))?;
        self.active = false;
        Ok(())
    }
}

impl Drop for TerminalMode {
    fn drop(&mut self) {
        if self.active && set_terminal_settings(self.fd, &self.original).is_ok() {
            self.active = false;
        }
    }
}

fn terminal_settings(fd: RawFd) -> io::Result<libc::termios> {
    let mut settings = MaybeUninit::<libc::termios>::uninit();
    // SAFETY: settings points to writable storage and fd remains open for this call.
    if unsafe { libc::tcgetattr(fd, settings.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: tcgetattr initialized settings after returning success.
    Ok(unsafe { settings.assume_init() })
}

fn set_terminal_settings(fd: RawFd, settings: &libc::termios) -> io::Result<()> {
    // SAFETY: settings is initialized and fd remains open for this call.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, settings) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn terminal_error(action: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("cannot {action}: {error}"))
}

struct Paths {
    fat: PathBuf,
    ini: PathBuf,
    app: PathBuf,
    manifest: PathBuf,
    test_mode: bool,
    /// Test-mode stand-in for the running kernel (emulated CI, host fixtures).
    kernel_release: Option<String>,
    test_keys: RefCell<VecDeque<InputEvent>>,
}

impl Paths {
    fn from_environment() -> Self {
        let fat =
            PathBuf::from(env::var_os("MISTER_MAGIK_FAT").unwrap_or_else(|| "/media/fat".into()));
        let public = ManifestLayout::Public.paths();
        let app = fat.join(
            Path::new(public.root)
                .strip_prefix("/media/fat")
                .expect("public app root is below /media/fat"),
        );
        let test_mode = env::var("MISTER_MAGIK_TEST_MODE").as_deref() == Ok("1");
        Self {
            ini: fat.join("MiSTer.ini"),
            manifest: app.join(mister_magik_platform_manifest_contract::FILE_NAME),
            test_mode,
            kernel_release: env::var("MISTER_MAGIK_TEST_KERNEL_RELEASE")
                .ok()
                .filter(|release| test_mode && !release.is_empty()),
            test_keys: RefCell::new(
                env::var("MISTER_MAGIK_TEST_KEYS")
                    .unwrap_or_default()
                    .split(',')
                    .filter(|key| !key.is_empty())
                    .map(input_event_from_key)
                    .collect(),
            ),
            app,
            fat,
        }
    }

    fn test_mode(&self) -> bool {
        self.test_mode
    }

    fn kernel_release(&self) -> Result<String> {
        match &self.kernel_release {
            Some(release) => Ok(release.clone()),
            None => running_kernel_release(),
        }
    }
}

fn input_event_from_key(key: &str) -> InputEvent {
    match key {
        "up" => InputEvent::Up,
        "down" => InputEvent::Down,
        "enter" => InputEvent::Confirm,
        "cancel" => InputEvent::Cancel,
        _ => InputEvent::Other,
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("MiSTer MagiK: ERROR: {error}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let paths = Paths::from_environment();
    let command = env::args().nth(1);
    match command.as_deref() {
        Some("status") => status(&paths),
        Some("verify-platform") => {
            verify_layout(&paths, start_layout(env::args().nth(2).as_deref())?)
        }
        Some("start") => start(&paths, start_layout(env::args().nth(2).as_deref())?),
        Some(LIVE_HANDOFF_COMMAND) => live_handoff(start_layout(env::args().nth(2).as_deref())?),
        Some(other) => Err(format!(
            "unknown command {other}; expected start, status, or verify-platform"
        )
        .into()),
        None => {
            Err("usage: mister-magik-manager start|verify-platform dev|public, or status".into())
        }
    }
}

fn status(paths: &Paths) -> Result<()> {
    let selected = effective(&paths.ini, "MiSTer", "main")?.unwrap_or_else(|| "<unset>".into());
    println!("MiSTer MagiK: effective Main={selected}");
    Ok(())
}

const STOCK_MAIN: &str = "/media/fat/MiSTer";
const SESSION_MAIN_ENV: &str = "MISTER_MAGIK_SESSION_MAIN";
const LIVE_HANDOFF_COMMAND: &str = "live-handoff";
const LIVE_HANDOFF_LOG: &str = "/tmp/mister-magik-start.log";
const LIVE_HANDOFF_LOCK: &str = "/tmp/mister-magik-start.lock";
// Lets the Scripts session release tty2 before MagiK starts its launcher there.
const LIVE_HANDOFF_DELAY: Duration = Duration::from_secs(2);
const STOCK_MAIN_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const SESSION_MAIN_START_TIMEOUT: Duration = Duration::from_secs(10);

fn start_layout(value: Option<&str>) -> Result<ManifestLayout> {
    let value = value.ok_or("start requires a layout: dev or public")?;
    ManifestLayout::parse(value)
        .map_err(|_| format!("unknown start layout {value}; expected dev or public").into())
}

fn layout_main_name(layout: ManifestLayout) -> &'static str {
    layout
        .paths()
        .main
        .rsplit('/')
        .next()
        .expect("layout Main has a file name")
}

fn fat_path(paths: &Paths, installed: &str) -> PathBuf {
    paths.fat.join(installed.trim_start_matches("/media/fat/"))
}

fn layout_app(paths: &Paths, layout: ManifestLayout) -> PathBuf {
    match layout {
        ManifestLayout::Public => paths.app.clone(),
        ManifestLayout::Development => fat_path(paths, layout.paths().root),
    }
}

fn layout_manifest(paths: &Paths, layout: ManifestLayout) -> PathBuf {
    match layout {
        ManifestLayout::Public => paths.manifest.clone(),
        ManifestLayout::Development => {
            layout_app(paths, layout).join(mister_magik_platform_manifest_contract::FILE_NAME)
        }
    }
}

/// Starts the layout's Main for this boot only. MiSTer.ini is never changed:
/// the fork keeps itself selected while `MISTER_MAGIK_SESSION_MAIN` names its
/// own executable, and Main's exec restarts for core launches and returns
/// inherit that environment. A reboot returns to the configured Main.
fn start(paths: &Paths, layout: ManifestLayout) -> Result<()> {
    let main = layout_main_name(layout);
    let running = if paths.test_mode() {
        None
    } else {
        running_magik_main()?
    };
    match running {
        Some(running) if running == main => {
            println!("MiSTer MagiK: {main} is already running.");
            return Ok(());
        }
        Some(running) => {
            return Err(format!(
                "{running} is running; only stock Main can hand off to {main} without rebooting"
            )
            .into());
        }
        None => {}
    }
    if !paths.test_mode() && try_lock(Path::new(LIVE_HANDOFF_LOCK))?.is_none() {
        return Err("a MagiK start is already in progress".into());
    }
    safety_confirmation(
        paths,
        &format!(
            "{main} will start now without rebooting. MiSTer.ini is not changed; rebooting returns to the configured Main."
        ),
        "start",
    )?;
    verify_layout(paths, layout)?;
    if !paths.test_mode() && !process_running("MiSTer")? {
        return Err("stock Main is not running; nothing was changed".into());
    }
    ensure_executable(fat_path(paths, layout.paths().main))?;
    ensure_executable(fat_path(paths, layout.paths().gui))?;
    sync_storage(paths)?;
    spawn_live_handoff(paths, layout)?;
    println!(
        "MiSTer MagiK: starting {main} in {} seconds. The screen goes black briefly.",
        LIVE_HANDOFF_DELAY.as_secs()
    );
    Ok(())
}

fn process_running(name: &str) -> Result<bool> {
    let output = Command::new("pidof").arg(name).output()?;
    Ok(output.stdout.iter().any(|byte| !byte.is_ascii_whitespace()))
}

fn running_magik_main() -> Result<Option<&'static str>> {
    for layout in [ManifestLayout::Development, ManifestLayout::Public] {
        let main = layout_main_name(layout);
        if process_running(main)? {
            return Ok(Some(main));
        }
    }
    Ok(None)
}

fn detached(command: &mut Command) -> &mut Command {
    // SAFETY: setsid is async-signal-safe and runs in the forked child only.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })
    }
}

fn spawn_live_handoff(paths: &Paths, layout: ManifestLayout) -> Result<()> {
    if paths.test_mode() {
        println!("MiSTer MagiK: TEST: live handoff requested.");
        return Ok(());
    }
    let log = File::create(LIVE_HANDOFF_LOG)?;
    let layout_arg = match layout {
        ManifestLayout::Public => "public",
        ManifestLayout::Development => "dev",
    };
    // The helper must outlive this Scripts session and stock Main.
    detached(
        Command::new(env::current_exe()?)
            .args([LIVE_HANDOFF_COMMAND, layout_arg])
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log),
    )
    .spawn()?;
    Ok(())
}

fn wait_until(timeout: Duration, mut done: impl FnMut() -> Result<bool>) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if done()? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn start_main(path: &str, session: bool) -> Result<()> {
    let mut command = Command::new(path);
    command
        .current_dir("/")
        .env_remove("MISTER_MAGIK_FAT")
        .env_remove("MISTER_MAGIK_TEST_MODE")
        .env_remove("MISTER_MAGIK_TEST_OUTPUT_MODE")
        .env_remove("MISTER_MAGIK_TEST_KEYS")
        .env_remove("MISTER_MAGIK_TEST_KERNEL_RELEASE")
        .env_remove(SESSION_MAIN_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if session {
        command.env(SESSION_MAIN_ENV, path);
    }
    detached(&mut command).spawn()?;
    Ok(())
}

/// Exclusive handoff lock. The kernel drops it when the holder exits, so a
/// crashed helper cannot leave a stale lock behind.
fn try_lock(path: &Path) -> Result<Option<File>> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    // SAFETY: flock only uses the descriptor number of the open file.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(file));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(None)
    } else {
        Err(error.into())
    }
}

/// Process operations behind the handoff, so its failure paths are testable.
trait Processes {
    fn running(&mut self, name: &str) -> Result<bool>;
    fn stop_stock(&mut self) -> Result<()>;
    fn start(&mut self, path: &str, session: bool) -> Result<()>;
    fn settle(&mut self);
}

struct SystemProcesses;

impl Processes for SystemProcesses {
    fn running(&mut self, name: &str) -> Result<bool> {
        process_running(name)
    }

    fn stop_stock(&mut self) -> Result<()> {
        let _ = Command::new("killall").args(["-TERM", "MiSTer"]).status();
        if !wait_until(STOCK_MAIN_STOP_TIMEOUT, || Ok(!process_running("MiSTer")?))? {
            let _ = Command::new("killall").args(["-KILL", "MiSTer"]).status();
            if !wait_until(Duration::from_secs(1), || Ok(!process_running("MiSTer")?))? {
                return Err("stock Main did not stop".into());
            }
        }
        Ok(())
    }

    fn start(&mut self, path: &str, session: bool) -> Result<()> {
        start_main(path, session)
    }

    fn settle(&mut self) {
        // Main replaces its own process while it loads the latch RBF, so callers
        // check by name after this bounded wait.
        std::thread::sleep(SESSION_MAIN_START_TIMEOUT);
    }
}

fn replace_stock_main(system: &mut dyn Processes, main_path: &str, main: &str) -> Result<()> {
    system.stop_stock()?;
    println!("MiSTer MagiK: stock Main stopped; starting {main_path}.");
    system.start(main_path, true)?;
    system.settle();
    if system.running(main)? {
        println!("MiSTer MagiK: {main} is running for this boot.");
        Ok(())
    } else {
        Err(format!("{main} did not stay running").into())
    }
}

/// Never starts a second Main: stock Main is restarted only when the process
/// table positively shows that no Main is running.
fn restore_stock_main(system: &mut dyn Processes, main: &str) -> Result<&'static str> {
    if system.running("MiSTer")? || system.running(main)? {
        return Ok("a Main is already running");
    }
    system.start(STOCK_MAIN, false)?;
    Ok("stock Main was restarted")
}

/// Replaces stock Main with the session Main. Every failure after stock Main
/// may have stopped goes through recovery, and both errors are reported.
fn run_handoff(system: &mut dyn Processes, main_path: &str, main: &str) -> Result<()> {
    for layout in [ManifestLayout::Development, ManifestLayout::Public] {
        let running = layout_main_name(layout);
        if system.running(running)? {
            return Err(format!("{running} is already running; nothing was changed").into());
        }
    }
    if !system.running("MiSTer")? {
        return Err("stock Main is not running; nothing was changed".into());
    }
    let Err(primary) = replace_stock_main(system, main_path, main) else {
        return Ok(());
    };
    println!("MiSTer MagiK: {primary}; recovering stock Main.");
    match restore_stock_main(system, main) {
        Ok(outcome) => Err(format!("{primary}; recovery: {outcome}").into()),
        Err(recovery) => Err(format!("{primary}; recovery failed: {recovery}").into()),
    }
}

/// Detached helper. Holds the handoff lock so concurrent starts cannot both
/// replace stock Main; a second helper does nothing.
fn live_handoff(layout: ManifestLayout) -> Result<()> {
    let Some(_lock) = try_lock(Path::new(LIVE_HANDOFF_LOCK))? else {
        return Err("another MagiK start is in progress; nothing was changed".into());
    };
    std::thread::sleep(LIVE_HANDOFF_DELAY);
    run_handoff(
        &mut SystemProcesses,
        layout.paths().main,
        layout_main_name(layout),
    )
}

fn safety_confirmation(paths: &Paths, message: &str, operation: &str) -> Result<()> {
    println!(
        "\n{message}\n\nPress Down on the keyboard or joystick to confirm. Any other input cancels."
    );
    match read_event(paths)? {
        Some(InputEvent::Down) => Ok(()),
        Some(_) => Err(format!("{operation} cancelled; no changes made").into()),
        None => Err(format!("interactive input is unavailable; {operation} refused").into()),
    }
}

fn read_event(paths: &Paths) -> Result<Option<InputEvent>> {
    if paths.test_mode() {
        return Ok(paths.test_keys.borrow_mut().pop_front());
    }
    if !io::stdin().is_terminal() {
        return Ok(None);
    }
    let stdin = io::stdin();
    let mut terminal = TerminalMode::enter(stdin.as_raw_fd())?;
    let result = read_event_fd(stdin.as_raw_fd());
    let restore = terminal.restore();
    println!();
    match (result, restore) {
        (Ok(event), Ok(())) => Ok(Some(event)),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Err(read_error), Err(restore_error)) => Err(format!(
            "{read_error}; additionally could not restore terminal settings: {restore_error}"
        )
        .into()),
    }
}

fn read_event_fd(fd: RawFd) -> Result<InputEvent> {
    let mut decoder = InputDecoder::default();
    let mut escape_started: Option<Instant> = None;
    loop {
        let timeout = if let Some(started) = escape_started {
            let remaining = Duration::from_millis(100).saturating_sub(started.elapsed());
            if remaining.is_zero() {
                log_rejected_escape(&decoder, started, "timeout");
                return Ok(InputEvent::Cancel);
            }
            remaining.as_millis().max(1) as i32
        } else {
            -1
        };
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: descriptor is valid writable storage for one pollfd.
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if ready == 0 {
            continue;
        }
        if let Some(started) = escape_started
            && started.elapsed() >= Duration::from_millis(100)
        {
            log_rejected_escape(&decoder, started, "timeout");
            return Ok(InputEvent::Cancel);
        }
        let mut byte = [0_u8; 1];
        // Unbuffered reads are essential: poll cannot see bytes held by StdinLock.
        // SAFETY: byte is writable storage and fd is borrowed for this call.
        let count = unsafe { libc::read(fd, byte.as_mut_ptr().cast(), 1) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if count == 0 {
            return Ok(InputEvent::Cancel);
        }
        if byte[0] == 0x1b && escape_started.is_none() {
            escape_started = Some(Instant::now());
        }
        if let Some(event) = decoder.push(&byte) {
            if event == InputEvent::Cancel
                && let Some(started) = escape_started
            {
                log_rejected_escape(&decoder, started, "unsupported");
            }
            return Ok(event);
        }
    }
}

fn log_rejected_escape(decoder: &InputDecoder, started: Instant, reason: &str) {
    let bytes = decoder
        .bytes
        .iter()
        .take(3)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!(
        "terminal_input rejected={reason} elapsed_ms={} escape_hex={bytes}",
        started.elapsed().as_millis()
    );
}

fn effective(path: &Path, section: &str, key: &str) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Document::parse(&fs::read(path)?)?.effective_value(section, key))
}

fn verify_layout(paths: &Paths, layout: ManifestLayout) -> Result<()> {
    let manifest = parse_layout_manifest(&layout_manifest(paths, layout), layout)?;
    let fields = manifest.values();
    for (name, expected) in layout.paths().components() {
        let local = fat_path(paths, expected);
        if digest(&local)? != fields[&format!("{name}_sha256")] {
            return Err(format!("hash mismatch for {}", local.display()).into());
        }
    }
    let app = layout_app(paths, layout);
    let module_metadata =
        parse_component_metadata(&app.join("mister_magik_scanout_slots.metadata.txt"))?;
    let latch_metadata =
        parse_component_metadata(&app.join("fpga/menu-magik-vblank-latch.metadata.txt"))?;
    if module_metadata.get("module_sha256") != Some(&fields["scanout_module_sha256"]) {
        return Err("scanout metadata module hash mismatch".into());
    }
    if latch_metadata.get("rbf_sha256") != Some(&fields["latch_rbf_sha256"]) {
        return Err("latch metadata RBF hash mismatch".into());
    }
    for metadata in [&module_metadata, &latch_metadata] {
        if metadata.get("platform_contract_sha256") != Some(&fields["platform_contract_sha256"]) {
            return Err("platform metadata contract mismatch".into());
        }
    }
    if latch_metadata.get("source_commit") != Some(&fields["menu_revision"]) {
        return Err("latch metadata source revision mismatch".into());
    }
    if latch_metadata.get("latch_protocol_version") != Some(&fields["latch_protocol_version"])
        || latch_metadata.get("latch_capability_mask") != Some(&fields["latch_capability_mask"])
    {
        return Err("latch metadata protocol identity mismatch".into());
    }
    let kernel = paths.kernel_release()?;
    if !module_matches_kernel(module_metadata.get("vermagic"), &kernel) {
        return Err(
            format!("scanout module vermagic does not match running kernel {kernel}").into(),
        );
    }
    // The frontend refuses platforms this rule rejects, so refuse them before
    // stock Main is stopped: 6.18 is Development-only.
    let metadata_release = module_metadata
        .get("kernel_release")
        .map_or(kernel.as_str(), String::as_str);
    let profile = (metadata_release == kernel)
        .then(|| {
            resolve_profile(
                &kernel,
                module_metadata.get("platform_profile").map(String::as_str),
                module_metadata.get("provider_identity").map(String::as_str),
                layout == ManifestLayout::Development,
            )
        })
        .flatten()
        .ok_or_else(|| format!("unsupported kernel/layout: {kernel} {layout:?}"))?;
    if profile == DEVELOPMENT_PROFILE
        && module_metadata.get("kernel_revision").map(String::as_str)
            != Some(DEVELOPMENT_KERNEL_REVISION)
    {
        return Err("scanout metadata kernel revision mismatch".into());
    }
    let main_path = fat_path(paths, layout.paths().main);
    if !main_supports_session(&main_path)? {
        return Err(format!(
            "{} predates no-reboot sessions ({SESSION_MAIN_ENV}); publish a platform with the session Main",
            main_path.display()
        )
        .into());
    }
    println!(
        "MiSTer MagiK: verified platform {}",
        fields["magik_revision"]
    );
    Ok(())
}

/// Main keeps a session across core restarts only if it carries the guard that
/// reads the session variable. The binary is hash-bound by the manifest.
fn main_supports_session(main: &Path) -> Result<bool> {
    let marker = SESSION_MAIN_ENV.as_bytes();
    Ok(fs::read(main)?
        .windows(marker.len())
        .any(|window| window == marker))
}

fn module_matches_kernel(vermagic: Option<&String>, kernel: &str) -> bool {
    vermagic.is_some_and(|value| value.starts_with(&format!("{kernel} ")))
}

/// The module can only load into the kernel that is running now.
fn running_kernel_release() -> Result<String> {
    let mut name = MaybeUninit::<libc::utsname>::uninit();
    // SAFETY: uname fully initializes the buffer when it returns zero.
    if unsafe { libc::uname(name.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: uname succeeded, and release is NUL-terminated.
    let release = unsafe { std::ffi::CStr::from_ptr(name.assume_init_ref().release.as_ptr()) };
    Ok(release.to_string_lossy().into_owned())
}

#[cfg(test)]
fn parse_manifest(path: &Path) -> Result<ParsedManifest> {
    parse_layout_manifest(path, ManifestLayout::Public)
}

fn parse_layout_manifest(path: &Path, layout: ManifestLayout) -> Result<ParsedManifest> {
    let text = fs::read_to_string(path)?;
    mister_magik_platform_manifest_contract::parse(&text, layout, ValidationProfile::ManagerLegacy)
        .map_err(|error| manager_manifest_error(&error).into())
}

fn manager_manifest_error(
    error: &mister_magik_platform_manifest_contract::ManifestError,
) -> String {
    match error.code() {
        "invalid_platform_manifest" if error.detail().starts_with("malformed line") => {
            "malformed platform manifest".to_string()
        }
        "invalid_platform_manifest" => "invalid platform manifest".to_string(),
        "invalid_platform_manifest_fields" => "platform manifest has unexpected fields".to_string(),
        "unsupported_platform_manifest" => "unsupported platform manifest".to_string(),
        "invalid_platform_release" => "invalid platform release identity".to_string(),
        "unsupported_latch_protocol" => {
            "platform does not provide the required latch v5 contract".to_string()
        }
        "platform_path_mismatch" => format!("invalid {}_path", error.detail()),
        "invalid_platform_identity" => format!("invalid {}", error.detail()),
        _ => error.to_string(),
    }
}

fn parse_component_metadata(path: &Path) -> Result<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    for (line_index, line) in fs::read_to_string(path)?.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line_number = line_index + 1;
        let (key, value) = line.split_once('=').ok_or_else(|| {
            format!(
                "malformed component metadata {}:{}: key '<unknown>' has no '='",
                path.display(),
                line_number
            )
        })?;
        if key.is_empty() || value.is_empty() {
            return Err(format!(
                "invalid component metadata {}:{}: key '{}' has an empty value",
                path.display(),
                line_number,
                key
            )
            .into());
        }
        if key == "source_status" {
            continue;
        }
        if fields.insert(key.into(), value.into()).is_some() {
            return Err(format!(
                "invalid component metadata {}:{}: duplicate key '{}'",
                path.display(),
                line_number,
                key
            )
            .into());
        }
    }
    Ok(fields)
}

fn digest(path: &Path) -> Result<String> {
    let output = Command::new("sha256sum").arg(path).output()?;
    if !output.status.success() {
        return Err(format!("sha256sum failed for {}", path.display()).into());
    }
    Ok(String::from_utf8(output.stdout)?
        .split_whitespace()
        .next()
        .ok_or("sha256sum returned no digest")?
        .to_string())
}

fn ensure_executable(path: PathBuf) -> Result<()> {
    let mut permissions = fs::metadata(&path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn sync_storage(paths: &Paths) -> Result<()> {
    if paths.test_mode() {
        return Ok(());
    }
    if !Command::new("sync").status()?.success() {
        return Err("sync failed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    fn fixture_paths(root: &Path) -> Paths {
        Paths {
            fat: root.to_path_buf(),
            ini: root.join("MiSTer.ini"),
            app: root.join("mister-magik"),
            manifest: root.join("manifest"),
            test_mode: true,
            kernel_release: Some("5.15.1-MiSTer".into()),
            test_keys: RefCell::default(),
        }
    }

    fn development_paths(root: &Path) -> Paths {
        Paths {
            kernel_release: Some("6.18.38-MiSTer".into()),
            ..fixture_paths(root)
        }
    }

    fn fixture_root(name: &str) -> PathBuf {
        let id = FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!("mister-manager-{name}-{}-{id}", process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn queue(paths: &Paths, events: impl IntoIterator<Item = InputEvent>) {
        paths.test_keys.borrow_mut().extend(events);
    }

    fn write_valid_platform(paths: &Paths) {
        write_layout_platform(paths, ManifestLayout::Public);
    }

    fn write_layout_platform(paths: &Paths, layout: ManifestLayout) {
        write_platform(paths, layout, layout == ManifestLayout::Development, true);
    }

    /// `development_kernel` selects 6.18 development-only module metadata;
    /// `session_main` selects a Main carrying the session guard marker.
    fn write_platform(
        paths: &Paths,
        layout: ManifestLayout,
        development_kernel: bool,
        session_main: bool,
    ) {
        let app = &layout_app(paths, layout);
        let fpga = app.join("fpga");
        fs::create_dir_all(&fpga).unwrap();
        let files = [
            (
                fat_path(paths, layout.paths().main),
                if session_main {
                    b"main MISTER_MAGIK_SESSION_MAIN".as_slice()
                } else {
                    b"main".as_slice()
                },
            ),
            (app.join("mister-magik-fb"), b"gui".as_slice()),
            (app.join("mister-magik-manager"), b"manager".as_slice()),
            (
                app.join("mister_magik_scanout_slots.ko"),
                b"module".as_slice(),
            ),
            (fpga.join("menu-magik-vblank-latch.rbf"), b"rbf".as_slice()),
        ];
        for (path, bytes) in &files {
            fs::write(path, bytes).unwrap();
        }

        let module_sha = digest(&app.join("mister_magik_scanout_slots.ko")).unwrap();
        let rbf_sha = digest(&fpga.join("menu-magik-vblank-latch.rbf")).unwrap();
        let contract = "1".repeat(64);
        let module_metadata = if development_kernel {
            format!(
                "kernel_release=6.18.38-MiSTer\nkernel_revision={DEVELOPMENT_KERNEL_REVISION}\nplatform_profile=stock-6.18-latch-reuse-v3\nprovider_identity=stock-6.18-latch-reuse-v3\ndevelopment_only=1\nmodule_sha256={module_sha}\nplatform_contract_sha256={contract}\nvermagic=6.18.38-MiSTer SMP mod_unload ARMv7 p2v8 \n"
            )
        } else {
            format!(
                "module_sha256={module_sha}\nplatform_contract_sha256={contract}\nvermagic=5.15.1-MiSTer SMP mod_unload ARMv7 p2v8 \n"
            )
        };
        fs::write(
            app.join("mister_magik_scanout_slots.metadata.txt"),
            module_metadata,
        )
        .unwrap();
        fs::write(
            fpga.join("menu-magik-vblank-latch.metadata.txt"),
            format!(
                "rbf_sha256={rbf_sha}\nplatform_contract_sha256={contract}\nsource_commit={}\nlatch_protocol_version=5\nlatch_capability_mask=0x03ff\n",
                "2".repeat(40)
            ),
        )
        .unwrap();

        let mut manifest = format!(
            "format=mister-magik-platform-v3\nplatform_release=platform-v0.7\nplatform_release_number=7\nplatform_bundle_id={}\nqualification_candidate_id={}\nlatch_protocol_version=5\nlatch_capability_mask=0x03ff\n",
            "3".repeat(64),
            "4".repeat(64)
        );
        for (name, installed_path) in layout.paths().components() {
            let local_path = paths
                .fat
                .join(installed_path.trim_start_matches("/media/fat/"));
            manifest.push_str(&format!("{name}_path={installed_path}\n"));
            manifest.push_str(&format!("{name}_sha256={}\n", digest(&local_path).unwrap()));
        }
        manifest.push_str(&format!(
            "platform_contract_sha256={contract}\nmain_revision={}\nmagik_revision={}\nmenu_revision={}\n",
            "5".repeat(40),
            "6".repeat(40),
            "2".repeat(40)
        ));
        let values = manifest
            .lines()
            .map(|line| {
                let (field, value) = line.split_once('=').unwrap();
                (field.to_owned(), value.to_owned())
            })
            .collect();
        manifest = manifest.replace(
            &format!("qualification_candidate_id={}", "4".repeat(64)),
            &format!(
                "qualification_candidate_id={}",
                mister_magik_platform_manifest_contract::qualification_candidate_id(&values)
            ),
        );
        fs::write(layout_manifest(paths, layout), manifest).unwrap();
    }

    fn pseudo_terminal() -> (OwnedFd, OwnedFd) {
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: openpty initializes both descriptors; unused optional outputs are null.
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, 0, "openpty failed: {}", io::Error::last_os_error());
        // SAFETY: openpty returned two newly owned descriptors.
        unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) }
    }

    fn assert_terminal_settings_eq(left: &libc::termios, right: &libc::termios) {
        assert_eq!(left.c_iflag, right.c_iflag);
        assert_eq!(left.c_oflag, right.c_oflag);
        assert_eq!(left.c_cflag, right.c_cflag);
        // PENDIN is transient kernel state, not a persisted terminal configuration bit.
        assert_eq!(left.c_lflag & !libc::PENDIN, right.c_lflag & !libc::PENDIN);
        assert_eq!(left.c_cc, right.c_cc);
        // SAFETY: both references point to initialized termios values.
        assert_eq!(unsafe { libc::cfgetispeed(left) }, unsafe {
            libc::cfgetispeed(right)
        });
        // SAFETY: both references point to initialized termios values.
        assert_eq!(unsafe { libc::cfgetospeed(left) }, unsafe {
            libc::cfgetospeed(right)
        });
    }

    #[test]
    fn terminal_mode_configures_timeout_and_explicitly_restores_pty() {
        let (_master, slave) = pseudo_terminal();
        let fd = slave.as_raw_fd();
        let original = terminal_settings(fd).unwrap();
        let mut terminal = TerminalMode::enter(fd).unwrap();

        let blocking = terminal_settings(fd).unwrap();
        assert_eq!(blocking.c_lflag & (libc::ECHO | libc::ICANON), 0);
        assert_eq!(blocking.c_cc[libc::VMIN], 1);
        assert_eq!(blocking.c_cc[libc::VTIME], 0);

        terminal.set_tail_timeout().unwrap();
        let timed = terminal_settings(fd).unwrap();
        assert_eq!(timed.c_lflag & (libc::ECHO | libc::ICANON), 0);
        assert_eq!(timed.c_cc[libc::VMIN], 0);
        assert_eq!(timed.c_cc[libc::VTIME], 1);

        terminal.restore().unwrap();
        assert_terminal_settings_eq(&terminal_settings(fd).unwrap(), &original);
    }

    #[test]
    fn terminal_mode_drop_restores_pty_after_early_return() {
        let (_master, slave) = pseudo_terminal();
        let fd = slave.as_raw_fd();
        let original = terminal_settings(fd).unwrap();
        {
            let mut terminal = TerminalMode::enter(fd).unwrap();
            terminal.set_tail_timeout().unwrap();
        }
        assert_terminal_settings_eq(&terminal_settings(fd).unwrap(), &original);
    }

    #[test]
    fn fragmented_keyboard_and_joystick_sequences_decode() {
        for chunks in [
            vec![b"\x1b".as_slice(), b"[".as_slice(), b"B".as_slice()],
            vec![b"\x1bO".as_slice(), b"B".as_slice()],
        ] {
            let mut decoder = InputDecoder::default();
            let mut event = None;
            for chunk in chunks {
                event = decoder.push(chunk).or(event);
            }
            assert_eq!(event, Some(InputEvent::Down));
        }
    }

    #[test]
    fn terminal_accepts_fragmented_arrows_and_restores_settings() {
        for sequence in [b"\x1b[B", b"\x1bOB", b"\x1b[A", b"\x1bOA"] {
            for split in 1..sequence.len() {
                let (master, slave) = pseudo_terminal();
                let original = terminal_settings(slave.as_raw_fd()).unwrap();
                let mode = TerminalMode::enter(slave.as_raw_fd()).unwrap();
                let bytes = *sequence;
                let writer = std::thread::spawn(move || {
                    let mut master = File::from(master);
                    master.write_all(&bytes[..split]).unwrap();
                    std::thread::sleep(Duration::from_millis(10));
                    master.write_all(&bytes[split..]).unwrap();
                    master
                });
                let expected = if sequence[2] == b'B' {
                    InputEvent::Down
                } else {
                    InputEvent::Up
                };
                assert_eq!(read_event_fd(slave.as_raw_fd()).unwrap(), expected);
                let _master = writer.join().unwrap();
                drop(mode);
                assert_terminal_settings_eq(
                    &original,
                    &terminal_settings(slave.as_raw_fd()).unwrap(),
                );
            }
        }
    }

    #[test]
    fn terminal_escape_timeout_is_one_deadline() {
        let (master, slave) = pseudo_terminal();
        let _mode = TerminalMode::enter(slave.as_raw_fd()).unwrap();
        let writer = std::thread::spawn(move || {
            let mut master = File::from(master);
            master.write_all(b"\x1b").unwrap();
            std::thread::sleep(Duration::from_millis(70));
            master.write_all(b"[").unwrap();
            master
        });
        let started = Instant::now();
        assert_eq!(
            read_event_fd(slave.as_raw_fd()).unwrap(),
            InputEvent::Cancel
        );
        let _master = writer.join().unwrap();
        assert!(started.elapsed() >= Duration::from_millis(95));
        assert!(started.elapsed() < Duration::from_millis(160));
    }

    #[test]
    fn decoder_distinguishes_navigation_confirmation_and_cancellation() {
        for (bytes, expected) in [
            (b"\x1b[A".as_slice(), InputEvent::Up),
            (b"\x1bOA".as_slice(), InputEvent::Up),
            (b"\n".as_slice(), InputEvent::Confirm),
            (b"\r".as_slice(), InputEvent::Confirm),
            (b"x".as_slice(), InputEvent::Other),
            (b"\x1bx".as_slice(), InputEvent::Cancel),
        ] {
            let mut decoder = InputDecoder::default();
            assert_eq!(decoder.push(bytes), Some(expected));
        }

        let mut controller_b = InputDecoder::default();
        assert_eq!(controller_b.push(b"\x1b"), None);
        assert_eq!(controller_b.finish(), Some(InputEvent::Cancel));

        let unavailable = InputDecoder::default();
        assert_eq!(unavailable.finish(), None);
    }

    #[test]
    fn manifest_parser_rejects_duplicate_fields() {
        let root = env::temp_dir().join(format!("mister-manager-manifest-{}", process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let manifest = root.join("platform-v3.manifest");
        fs::write(&manifest, b"format=one\nformat=two\n").unwrap();
        assert!(parse_manifest(&manifest).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn component_metadata_accepts_repeated_source_status_entries() {
        let root = fixture_root("component-metadata-repeated-source-status");
        let metadata = root.join("latch.metadata.txt");
        fs::write(
            &metadata,
            b"format=fixture\nsource_status= M menu.qsf\nsource_status= M sys/sys_top.sdc\n",
        )
        .unwrap();

        let fields = parse_component_metadata(&metadata).unwrap();
        assert_eq!(fields.get("format"), Some(&"fixture".to_string()));
        assert!(!fields.contains_key("source_status"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn component_metadata_accepts_single_source_status_entry() {
        let root = fixture_root("component-metadata-single-source-status");
        let metadata = root.join("latch.metadata.txt");
        fs::write(&metadata, b"source_status= M sys/sys_top.sdc\n").unwrap();

        assert!(parse_component_metadata(&metadata).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn component_metadata_rejects_empty_source_status_with_location() {
        let root = fixture_root("component-metadata-empty-source-status");
        let metadata = root.join("latch.metadata.txt");
        fs::write(&metadata, b"source_status=\n").unwrap();

        let error = parse_component_metadata(&metadata).unwrap_err().to_string();
        assert!(error.contains(&metadata.display().to_string()));
        assert!(error.contains(":1:"));
        assert!(error.contains("key 'source_status'"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn component_metadata_rejects_duplicate_nonrepeatable_key_with_location() {
        let root = fixture_root("component-metadata-duplicate-key");
        let metadata = root.join("latch.metadata.txt");
        fs::write(&metadata, b"format=one\nformat=two\n").unwrap();

        let error = parse_component_metadata(&metadata).unwrap_err().to_string();
        assert!(error.contains(&metadata.display().to_string()));
        assert!(error.contains(":2:"));
        assert!(error.contains("duplicate key 'format'"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn component_metadata_rejects_malformed_line_with_location() {
        let root = fixture_root("component-metadata-malformed-line");
        let metadata = root.join("latch.metadata.txt");
        fs::write(&metadata, b"not-a-key-value-line\n").unwrap();

        let error = parse_component_metadata(&metadata).unwrap_err().to_string();
        assert!(error.contains(&metadata.display().to_string()));
        assert!(error.contains(":1:"));
        assert!(error.contains("key '<unknown>'"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn start_dev_leaves_boot_configuration_and_public_state_unchanged() {
        let root = fixture_root("start-dev");
        let paths = development_paths(&root);
        let ini = b"[MiSTer]\nmain=MiSTer\nvideo_mode=8\n";
        fs::write(&paths.ini, ini).unwrap();
        write_layout_platform(&paths, ManifestLayout::Development);
        queue(&paths, [InputEvent::Down]);

        start(&paths, ManifestLayout::Development).unwrap();
        assert_eq!(fs::read(&paths.ini).unwrap(), ini);
        assert!(!paths.app.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn start_refuses_without_confirmation_or_valid_platform() {
        let root = fixture_root("start-refused");
        let paths = development_paths(&root);
        write_layout_platform(&paths, ManifestLayout::Development);
        queue(&paths, [InputEvent::Confirm]);
        let error = start(&paths, ManifestLayout::Development).unwrap_err();
        assert!(error.to_string().contains("start cancelled"));

        // A Public-only install does not satisfy a Development start.
        let public_root = fixture_root("start-public-only");
        let public_paths = development_paths(&public_root);
        write_valid_platform(&public_paths);
        queue(&public_paths, [InputEvent::Down]);
        assert!(start(&public_paths, ManifestLayout::Development).is_err());

        fs::write(
            layout_manifest(&paths, ManifestLayout::Development),
            b"format=unsupported\n",
        )
        .unwrap();
        queue(&paths, [InputEvent::Down]);
        assert!(start(&paths, ManifestLayout::Development).is_err());

        // A module built for another kernel cannot load into the running one.
        let module = |value: &str| Some(value.to_owned());
        assert!(module_matches_kernel(
            module("6.18.38-MiSTer SMP mod_unload ARMv7 p2v8 ").as_ref(),
            "6.18.38-MiSTer"
        ));
        assert!(!module_matches_kernel(
            module("5.15.1-MiSTer SMP").as_ref(),
            "6.18.38-MiSTer"
        ));
        assert!(!module_matches_kernel(
            module("6.18.38-MiSTer2 SMP").as_ref(),
            "6.18.38-MiSTer"
        ));
        assert!(!module_matches_kernel(None, "6.18.38-MiSTer"));

        assert!(start_layout(None).is_err());
        assert!(start_layout(Some("stock")).is_err());
        assert_eq!(
            layout_main_name(ManifestLayout::Development),
            "MiSTer_MagiKDev"
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(public_root).unwrap();
    }

    #[test]
    fn public_start_accepts_legacy_and_refuses_development_only_kernels() {
        let root = fixture_root("public-kernels");
        let paths = fixture_paths(&root);
        write_valid_platform(&paths);
        verify_layout(&paths, ManifestLayout::Public).unwrap();

        // 6.18 platforms are Development-only, so a public start must refuse
        // before stock Main is stopped.
        let root618 = fixture_root("public-618");
        let mut paths618 = development_paths(&root618);
        write_platform(&paths618, ManifestLayout::Public, true, true);
        let error = verify_layout(&paths618, ManifestLayout::Public).unwrap_err();
        assert!(error.to_string().contains("unsupported kernel/layout"));
        queue(&paths618, [InputEvent::Down]);
        assert!(
            start(&paths618, ManifestLayout::Public)
                .unwrap_err()
                .to_string()
                .contains("unsupported kernel/layout")
        );

        // The same module is accepted for the Development layout.
        let dev_root = fixture_root("dev-618");
        let dev = development_paths(&dev_root);
        write_layout_platform(&dev, ManifestLayout::Development);
        verify_layout(&dev, ManifestLayout::Development).unwrap();

        // A module built for another kernel than the running one is refused.
        paths618.kernel_release = Some("5.15.1-MiSTer".into());
        write_platform(&paths618, ManifestLayout::Development, true, true);
        assert!(verify_layout(&paths618, ManifestLayout::Development).is_err());

        // Wrong development profile revision is refused.
        let metadata = layout_app(&dev, ManifestLayout::Development)
            .join("mister_magik_scanout_slots.metadata.txt");
        let text = fs::read_to_string(&metadata).unwrap();
        fs::write(
            &metadata,
            text.replace(DEVELOPMENT_KERNEL_REVISION, &"0".repeat(40)),
        )
        .unwrap();
        assert!(verify_layout(&dev, ManifestLayout::Development).is_err());

        for root in [root, root618, dev_root] {
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn main_without_the_session_guard_is_refused_before_anything_stops() {
        let root = fixture_root("no-session-main");
        let paths = fixture_paths(&root);
        write_platform(&paths, ManifestLayout::Public, false, false);
        let error = verify_layout(&paths, ManifestLayout::Public).unwrap_err();
        assert!(error.to_string().contains("predates no-reboot sessions"));
        queue(&paths, [InputEvent::Down]);
        assert!(start(&paths, ManifestLayout::Public).is_err());

        write_valid_platform(&paths);
        verify_layout(&paths, ManifestLayout::Public).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn handoff_lock_admits_one_holder_at_a_time() {
        let root = fixture_root("lock");
        let path = root.join("start.lock");
        let first = try_lock(&path).unwrap();
        assert!(first.is_some());
        assert!(try_lock(&path).unwrap().is_none());
        drop(first);
        assert!(try_lock(&path).unwrap().is_some());
        fs::remove_dir_all(root).unwrap();
    }

    struct FakeProcesses {
        running: Vec<&'static str>,
        stop_error: bool,
        start_error: Option<&'static str>,
        check_error_after_start: bool,
        session_survives: bool,
        log: Vec<String>,
        started: bool,
    }

    impl FakeProcesses {
        fn stock_only() -> Self {
            Self {
                running: vec!["MiSTer"],
                stop_error: false,
                start_error: None,
                check_error_after_start: false,
                session_survives: true,
                log: Vec::new(),
                started: false,
            }
        }
    }

    impl Processes for FakeProcesses {
        fn running(&mut self, name: &str) -> Result<bool> {
            if self.check_error_after_start && self.started {
                return Err("pidof failed".into());
            }
            Ok(self.running.contains(&name))
        }

        fn stop_stock(&mut self) -> Result<()> {
            self.log.push("stop".into());
            if self.stop_error {
                return Err("stock Main did not stop".into());
            }
            self.running.retain(|name| *name != "MiSTer");
            Ok(())
        }

        fn start(&mut self, path: &str, session: bool) -> Result<()> {
            self.log.push(format!("start {path} session={session}"));
            if let Some(error) = self.start_error.filter(|_| session) {
                return Err(error.into());
            }
            self.started = true;
            if session {
                if self.session_survives {
                    self.running.push("MiSTer_MagiKDev");
                }
            } else {
                self.running.push("MiSTer");
            }
            Ok(())
        }

        fn settle(&mut self) {
            self.log.push("settle".into());
        }
    }

    const DEV_MAIN: &str = "/media/fat/MiSTer_MagiKDev";

    #[test]
    fn handoff_replaces_stock_main_without_recovery() {
        let mut system = FakeProcesses::stock_only();
        run_handoff(&mut system, DEV_MAIN, "MiSTer_MagiKDev").unwrap();
        assert_eq!(
            system.log,
            [
                "stop",
                "start /media/fat/MiSTer_MagiKDev session=true",
                "settle"
            ]
        );
    }

    #[test]
    fn handoff_recovers_stock_main_after_every_post_stop_failure() {
        // The session Main cannot be spawned.
        let mut system = FakeProcesses::stock_only();
        system.start_error = Some("spawn failed");
        let error = run_handoff(&mut system, DEV_MAIN, "MiSTer_MagiKDev").unwrap_err();
        let text = error.to_string();
        assert!(text.contains("spawn failed") && text.contains("stock Main was restarted"));
        assert!(system.running.contains(&"MiSTer"));

        // The session Main dies during the settle window.
        let mut system = FakeProcesses::stock_only();
        system.session_survives = false;
        let error = run_handoff(&mut system, DEV_MAIN, "MiSTer_MagiKDev").unwrap_err();
        assert!(error.to_string().contains("did not stay running"));
        assert!(system.running.contains(&"MiSTer"));

        // The post-start process check itself fails: no recovery target can be
        // proven, so nothing more is started and both errors are reported.
        let mut system = FakeProcesses::stock_only();
        system.check_error_after_start = true;
        let error = run_handoff(&mut system, DEV_MAIN, "MiSTer_MagiKDev").unwrap_err();
        let text = error.to_string();
        assert!(text.contains("pidof failed") && text.contains("recovery failed"));
        assert_eq!(
            system
                .log
                .iter()
                .filter(|line| line.contains("start"))
                .count(),
            1
        );
    }

    #[test]
    fn handoff_never_starts_a_second_main() {
        // Stock Main would not stop: it is still running, so recovery is a no-op.
        let mut system = FakeProcesses::stock_only();
        system.stop_error = true;
        let error = run_handoff(&mut system, DEV_MAIN, "MiSTer_MagiKDev").unwrap_err();
        assert!(error.to_string().contains("a Main is already running"));
        assert_eq!(system.log, ["stop"]);

        // A concurrent start already brought MagiK up: nothing is changed.
        let mut system = FakeProcesses::stock_only();
        system.running.push("MiSTer_MagiKDev");
        let error = run_handoff(&mut system, DEV_MAIN, "MiSTer_MagiKDev").unwrap_err();
        assert!(error.to_string().contains("already running"));
        assert!(system.log.is_empty());

        // Stock Main is gone before the helper runs: nothing is changed.
        let mut system = FakeProcesses::stock_only();
        system.running.clear();
        assert!(run_handoff(&mut system, DEV_MAIN, "MiSTer_MagiKDev").is_err());
        assert!(system.log.is_empty());
    }

    #[test]
    fn manifest_validation_rejects_malformed_and_noncanonical_hex() {
        let root = fixture_root("manifest-errors");
        let paths = fixture_paths(&root);
        fs::write(&paths.manifest, b"missing-separator\n").unwrap();
        assert_eq!(
            parse_manifest(&paths.manifest).unwrap_err().to_string(),
            "malformed platform manifest"
        );

        write_valid_platform(&paths);
        let manifest = fs::read_to_string(&paths.manifest).unwrap().replace(
            &format!("platform_bundle_id={}", "3".repeat(64)),
            &format!("platform_bundle_id={}", "A".repeat(64)),
        );
        fs::write(&paths.manifest, manifest).unwrap();
        assert_eq!(
            parse_manifest(&paths.manifest).unwrap_err().to_string(),
            format!("invalid platform_bundle_id: {}", "A".repeat(64))
        );

        write_valid_platform(&paths);
        let manifest = fs::read_to_string(&paths.manifest).unwrap();
        let candidate = manifest
            .lines()
            .find(|line| line.starts_with("qualification_candidate_id="))
            .unwrap();
        fs::write(
            &paths.manifest,
            manifest.replace(
                candidate,
                &format!("qualification_candidate_id={}", "f".repeat(64)),
            ),
        )
        .unwrap();
        assert_eq!(
            parse_manifest(&paths.manifest).unwrap_err().to_string(),
            format!("platform_candidate_identity_mismatch: {}", "f".repeat(64))
        );
        fs::remove_dir_all(root).unwrap();
    }
}
