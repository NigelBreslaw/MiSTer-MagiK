// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_ini::{Document, apply_install, apply_restore};
use mister_magik_platform_manifest_contract::{
    Layout as ManifestLayout, ParsedManifest, ValidationProfile,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Restore,
    Uninstall,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WriteStep {
    BeforeCreate,
    AfterWrite,
    AfterFlush,
    AfterPendingReadback,
    AfterRename,
    AfterFinalReadback,
    AfterDirectorySync,
}

trait WriteFaults {
    fn check(&mut self, _path: &Path, _step: WriteStep) -> io::Result<()> {
        Ok(())
    }
}

struct NoWriteFaults;
impl WriteFaults for NoWriteFaults {}

struct PreparedFile {
    path: PathBuf,
    original: Option<Vec<u8>>,
    replacement: Vec<u8>,
}

impl PreparedFile {
    fn new(path: PathBuf, replacement: Vec<u8>) -> Result<Self> {
        let original = if path.exists() {
            Some(fs::read(&path)?)
        } else {
            None
        };
        Ok(Self {
            path,
            original,
            replacement,
        })
    }
}

struct Paths {
    fat: PathBuf,
    inittab: PathBuf,
    ini: PathBuf,
    backup: PathBuf,
    app: PathBuf,
    manifest: PathBuf,
    script: PathBuf,
    script_constants: PathBuf,
    test_mode: bool,
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
        Self {
            inittab: PathBuf::from(
                env::var_os("MISTER_MAGIK_INITTAB").unwrap_or_else(|| "/etc/inittab".into()),
            ),
            ini: fat.join("MiSTer.ini"),
            backup: fat.join("MiSTer.ini.bak.before-magik"),
            manifest: app.join(mister_magik_platform_manifest_contract::FILE_NAME),
            script: fat.join("Scripts/MiSTer-MagiK.sh"),
            script_constants: fat.join("Scripts/MiSTer-MagiK.platform-v3.constants.sh"),
            test_mode: env::var("MISTER_MAGIK_TEST_MODE").as_deref() == Ok("1"),
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
        Some("verify-platform") => verify_platform(&paths),
        Some("install") => install(&paths),
        Some("restore") => restore(&paths),
        Some("uninstall") => uninstall(&paths),
        Some(other) => Err(format!("unknown command {other}; expected install, restore, uninstall, status, or verify-platform").into()),
        None => {
            if selects_magik(&paths.ini)? {
                match choose_installed_action(&paths)? {
                    Some(Action::Restore) => restore(&paths),
                    Some(Action::Uninstall) => uninstall(&paths),
                    _ => Ok(()),
                }
            } else { install(&paths) }
        }
    }
}

fn status(paths: &Paths) -> Result<()> {
    let selected = effective(&paths.ini, "MiSTer", "main")?.unwrap_or_else(|| "<unset>".into());
    println!("MiSTer MagiK: effective Main={selected}");
    Ok(())
}

fn install(paths: &Paths) -> Result<()> {
    safety_confirmation(
        paths,
        "MiSTer MagiK will become the selected Main. Existing video and output settings will not be changed.",
        "installation",
    )?;
    verify_platform(paths).map_err(|error| {
        format!("platform verification failed; boot configuration was not changed: {error}")
    })?;
    snapshot(paths)?;
    backup_ini(paths)?;
    ensure_executable(paths.fat.join("MiSTer_MagiK"))?;
    ensure_executable(paths.app.join("mister-magik-fb"))?;
    ensure_executable(paths.app.join("mister-magik-manager"))?;
    remount_root_writable(paths)?;
    let ini = prepare_ini(&paths.ini, apply_install)?;
    let inittab = prepare_stock_inittab(&paths.inittab)?;
    let files = vec![
        PreparedFile::new(paths.inittab.clone(), inittab)?,
        PreparedFile::new(paths.ini.clone(), ini)?,
    ];
    replace_transaction(paths, &files, &mut NoWriteFaults, || {
        validate_install(paths)
    })?;
    println!("MiSTer MagiK: installed. Rebooting to start MiSTer MagiK.");
    reboot_now(paths)
}

fn restore(paths: &Paths) -> Result<()> {
    restore_stock(paths)?;
    println!("MiSTer MagiK: stock MiSTer boot restored. MiSTer MagiK files were preserved.");
    offer_reboot(paths)
}

fn uninstall(paths: &Paths) -> Result<()> {
    safety_confirmation(
        paths,
        "This permanently removes MiSTer MagiK, its settings, catalog, downloaded media, installer scripts, update_all entry, and saved backup. Stock MiSTer boot will be restored first.",
        "uninstall",
    )?;
    restore_stock(paths)?;
    stop_children(paths)?;
    remove_owned(paths)?;
    println!("MiSTer MagiK: fully uninstalled.");
    offer_reboot(paths)
}

fn restore_stock(paths: &Paths) -> Result<()> {
    snapshot(paths)?;
    remount_root_writable(paths)?;
    let backup = if paths.backup.is_file() {
        Some(Document::parse(&fs::read(&paths.backup)?)?)
    } else {
        None
    };
    let ini = prepare_ini(&paths.ini, |document| {
        apply_restore(document, backup.as_ref())
    })?;
    let inittab = prepare_stock_inittab(&paths.inittab)?;
    let files = vec![
        PreparedFile::new(paths.inittab.clone(), inittab)?,
        PreparedFile::new(paths.ini.clone(), ini)?,
    ];
    replace_transaction(paths, &files, &mut NoWriteFaults, || validate_stock(paths))
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

fn choose_installed_action(paths: &Paths) -> Result<Option<Action>> {
    let mut action = Action::Restore;
    loop {
        println!(
            "MiSTer MagiK is installed and selected as Main.\nUse Up/Down to choose, A/Enter to continue, or B/Escape to cancel."
        );
        println!(
            "{} Restore stock MiSTer\n{} Fully uninstall MiSTer MagiK",
            if action == Action::Restore { '>' } else { ' ' },
            if action == Action::Uninstall {
                '>'
            } else {
                ' '
            }
        );
        match read_event(paths)? {
            Some(InputEvent::Up | InputEvent::Down) => {
                action = if action == Action::Restore {
                    Action::Uninstall
                } else {
                    Action::Restore
                }
            }
            Some(InputEvent::Confirm) => return Ok(Some(action)),
            Some(InputEvent::Cancel | InputEvent::Other) => return Ok(None),
            None => return Ok(None),
        }
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
    let result = read_event_bytes(&mut stdin.lock(), &mut terminal);
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

fn read_event_bytes(input: &mut impl Read, terminal: &mut TerminalMode) -> Result<InputEvent> {
    let mut first = [0_u8; 1];
    input.read_exact(&mut first)?;
    let mut decoder = InputDecoder::default();
    if let Some(event) = decoder.push(&first) {
        return Ok(event);
    }
    let mut tail = [0_u8; 2];
    terminal.set_tail_timeout()?;
    let count = input.read(&mut tail)?;
    decoder
        .push(&tail[..count])
        .or_else(|| decoder.finish())
        .ok_or_else(|| "interactive input ended before a complete event".into())
}

fn effective(path: &Path, section: &str, key: &str) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Document::parse(&fs::read(path)?)?.effective_value(section, key))
}

fn selects_magik(path: &Path) -> Result<bool> {
    Ok(effective(path, "MiSTer", "main")?.as_deref() == Some("MiSTer_MagiK"))
}

fn prepare_ini(path: &Path, mutation: impl FnOnce(&mut Document)) -> Result<Vec<u8>> {
    let input = if path.is_file() {
        fs::read(path)?
    } else {
        Vec::new()
    };
    let mut document = Document::parse(&input)?;
    mutation(&mut document);
    let output = document.render();
    Document::parse(&output)?;
    Ok(output)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with_faults(path, bytes, &mut NoWriteFaults)
}

fn atomic_write_with_faults(path: &Path, bytes: &[u8], faults: &mut dyn WriteFaults) -> Result<()> {
    let parent = path.parent().ok_or("target has no parent")?;
    fs::create_dir_all(parent)?;
    let pending = parent.join(format!(
        ".{}.new.{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or("invalid filename")?,
        process::id()
    ));
    let result = (|| -> Result<()> {
        faults.check(path, WriteStep::BeforeCreate)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)?;
        file.write_all(bytes)?;
        faults.check(path, WriteStep::AfterWrite)?;
        file.sync_all()?;
        faults.check(path, WriteStep::AfterFlush)?;
        drop(file);
        if fs::read(&pending)? != bytes {
            return Err("pending file read-back mismatch".into());
        }
        faults.check(path, WriteStep::AfterPendingReadback)?;
        fs::rename(&pending, path)?;
        faults.check(path, WriteStep::AfterRename)?;
        if fs::read(path)? != bytes {
            return Err("replaced file read-back mismatch".into());
        }
        faults.check(path, WriteStep::AfterFinalReadback)?;
        File::open(parent)?.sync_all()?;
        faults.check(path, WriteStep::AfterDirectorySync)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&pending);
    }
    result
}

fn replace_transaction(
    paths: &Paths,
    files: &[PreparedFile],
    faults: &mut dyn WriteFaults,
    validate: impl FnOnce() -> Result<()>,
) -> Result<()> {
    for (index, file) in files.iter().enumerate() {
        let write_result = atomic_write_with_faults(&file.path, &file.replacement, faults);
        if let Err(error) = write_result {
            rollback_files(&files[..=index])?;
            return Err(format!(
                "cannot replace {}: {error}; rollback=complete",
                file.path.display()
            )
            .into());
        }
    }
    let finish = sync_storage(paths).and_then(|()| validate());
    if let Err(error) = finish {
        rollback_files(files)?;
        sync_storage(paths)?;
        return Err(
            format!("boot configuration validation failed: {error}; rollback=complete").into(),
        );
    }
    Ok(())
}

fn rollback_files(files: &[PreparedFile]) -> Result<()> {
    for file in files.iter().rev() {
        match &file.original {
            Some(bytes) => atomic_write(&file.path, bytes)?,
            None if file.path.exists() => fs::remove_file(&file.path)?,
            None => {}
        }
    }
    Ok(())
}

fn backup_ini(paths: &Paths) -> Result<()> {
    if !paths.ini.is_file() || paths.backup.exists() {
        return Ok(());
    }
    if selects_magik(&paths.ini)? {
        println!(
            "MiSTer MagiK: WARNING: backup missing; not creating it from a MagiK-active MiSTer.ini."
        );
        return Ok(());
    }
    atomic_write(&paths.backup, &fs::read(&paths.ini)?)
}

fn prepare_stock_inittab(path: &Path) -> Result<Vec<u8>> {
    let input = fs::read_to_string(path)?;
    let newline = if input.contains("\r\n") { "\r\n" } else { "\n" };
    let mut output = Vec::new();
    let mut wrote = false;
    let (magik_main, magik_boot) = magik_inittab_prefixes();
    for raw in input.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with("::sysinit:/media/fat/MiSTer ") && line.ends_with('&') {
            if !wrote {
                output.push("::sysinit:/media/fat/MiSTer &");
                wrote = true;
            }
        } else if !line.starts_with(&magik_main) && !line.starts_with(&magik_boot) {
            output.push(line);
        }
    }
    if !wrote {
        output.push("::sysinit:/media/fat/MiSTer &");
    }
    let mut bytes = output.join(newline).into_bytes();
    bytes.extend_from_slice(newline.as_bytes());
    Ok(bytes)
}

fn magik_inittab_prefixes() -> (String, String) {
    let public = ManifestLayout::Public.paths();
    (
        format!("::sysinit:{}", public.main),
        format!("::sysinit:{}/boot.sh", public.root),
    )
}

fn remount_root_writable(paths: &Paths) -> Result<()> {
    if paths.test_mode() {
        return Ok(());
    }
    let status = Command::new("mount")
        .args(["-o", "remount,rw", "/"])
        .status()?;
    if !status.success() {
        return Err("cannot remount root filesystem writable".into());
    }
    Ok(())
}

fn validate_install(paths: &Paths) -> Result<()> {
    let document = Document::parse(&fs::read(&paths.ini)?)?;
    if document.active_count("MiSTer", "main") != 1
        || document.effective_value("MiSTer", "main").as_deref() != Some("MiSTer_MagiK")
    {
        return Err("MiSTer.main did not validate".into());
    }
    verify_stock_inittab(&paths.inittab)
}

fn validate_stock(paths: &Paths) -> Result<()> {
    let document = Document::parse(&fs::read(&paths.ini)?)?;
    if document.effective_value("MiSTer", "main").as_deref() == Some("MiSTer_MagiK") {
        return Err("MiSTer.ini still selects MiSTer MagiK".into());
    }
    if document.active_count("MiSTer", "main") > 1 {
        return Err("MiSTer.main remains duplicated".into());
    }
    verify_stock_inittab(&paths.inittab)
}

fn verify_stock_inittab(path: &Path) -> Result<()> {
    let text = fs::read_to_string(path)?;
    let (magik_main, magik_boot) = magik_inittab_prefixes();
    let stock = text
        .lines()
        .filter(|line| line.trim_end_matches('\r') == "::sysinit:/media/fat/MiSTer &")
        .count();
    if stock != 1
        || text
            .lines()
            .any(|line| line.starts_with(&magik_main) || line.starts_with(&magik_boot))
    {
        return Err("inittab is not in verified stock state".into());
    }
    Ok(())
}

fn verify_platform(paths: &Paths) -> Result<()> {
    let manifest = parse_manifest(&paths.manifest)?;
    let fields = manifest.values();
    for (name, expected) in ManifestLayout::Public.paths().components() {
        let local = paths.fat.join(expected.trim_start_matches("/media/fat/"));
        if digest(&local)? != fields[&format!("{name}_sha256")] {
            return Err(format!("hash mismatch for {}", local.display()).into());
        }
    }
    let module_metadata =
        parse_component_metadata(&paths.app.join("mister_magik_scanout_slots.metadata.txt"))?;
    let latch_metadata =
        parse_component_metadata(&paths.app.join("fpga/menu-magik-vblank-latch.metadata.txt"))?;
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
    if !module_metadata
        .get("vermagic")
        .is_some_and(|value| value.starts_with("5.15.1-MiSTer "))
    {
        return Err("scanout module vermagic is incompatible".into());
    }
    println!(
        "MiSTer MagiK: verified platform {}",
        fields["magik_revision"]
    );
    Ok(())
}

fn parse_manifest(path: &Path) -> Result<ParsedManifest> {
    let text = fs::read_to_string(path)?;
    mister_magik_platform_manifest_contract::parse(
        &text,
        ManifestLayout::Public,
        ValidationProfile::ManagerLegacy,
    )
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

fn snapshot(paths: &Paths) -> Result<()> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let directory = paths.app.join("snapshots").join(format!("{stamp}-manager"));
    fs::create_dir_all(&directory)?;
    for (source, name) in [(&paths.inittab, "inittab"), (&paths.ini, "MiSTer.ini")] {
        if source.is_file() {
            fs::copy(source, directory.join(name))?;
        }
    }
    if let Ok(output) = Command::new("ps").output()
        && output.status.success()
    {
        fs::write(directory.join("ps.txt"), output.stdout)?;
    }
    for (source, name) in [
        (
            Path::new("/sys/module/MiSTer_fb/parameters/mode"),
            "fb-mode.txt",
        ),
        (
            Path::new("/tmp/mister-magik-main.log"),
            "mister-magik-main.log",
        ),
        (Path::new("/tmp/mister-magik/status.json"), "status.json"),
    ] {
        if source.is_file() {
            let _ = fs::copy(source, directory.join(name));
        }
    }
    println!("MiSTer MagiK: snapshot: {}", directory.display());
    Ok(())
}

fn ensure_executable(path: PathBuf) -> Result<()> {
    let mut permissions = fs::metadata(&path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn stop_children(paths: &Paths) -> Result<()> {
    if paths.test_mode() {
        return Ok(());
    }
    let output = Command::new("pidof").arg("mister-magik-fb").output()?;
    for pid in String::from_utf8_lossy(&output.stdout).split_whitespace() {
        let _ = Command::new("kill").args(["-TERM", pid]).status();
    }
    std::thread::sleep(Duration::from_secs(1));
    let output = Command::new("pidof").arg("mister-magik-fb").output()?;
    for pid in String::from_utf8_lossy(&output.stdout).split_whitespace() {
        let _ = Command::new("kill").args(["-KILL", pid]).status();
    }
    let remaining = Command::new("pidof").arg("mister-magik-fb").output()?;
    if remaining
        .stdout
        .iter()
        .any(|byte| !byte.is_ascii_whitespace())
    {
        Err("mister-magik-fb did not stop within the bounded timeout".into())
    } else {
        Ok(())
    }
}

fn remove_owned(paths: &Paths) -> Result<()> {
    let files = vec![
        paths.fat.join("MiSTer_MagiK"),
        paths.fat.join("Scripts/mister-magik.sh"),
        paths.fat.join("Scripts/mister-magik-channel.sh"),
        paths.fat.join("downloader_mister_magik.ini"),
        paths.backup.clone(),
        paths.fat.join("THIRD-PARTY-NOTICES.txt"),
        paths.fat.join("SOURCE-OFFER.txt"),
        paths.fat.join("licenses/MiSTer-MagiK-GPL-3.0-or-later.txt"),
        paths.fat.join("licenses/RUST-LIBRARIES.txt"),
        paths.fat.join("licenses/FFMPEG-LGPL-2.1-or-later.txt"),
        paths.fat.join("licenses/PRESS-START-2P-OFL-1.1.txt"),
        paths.fat.join("licenses/ARCADE-CABINET-CC-BY-NC-4.0.txt"),
    ];
    for path in &files {
        if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    let mut stale = Vec::new();
    for entry in fs::read_dir(&paths.fat)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("downloader_mister_magik.ini.tmp.")
            || name.starts_with(".downloader_mister_magik.ini")
            || name.starts_with(".MiSTer.ini.bak.before-magik.new.")
            || name.starts_with(".MiSTer.ini.magik.new")
        {
            stale.push(entry.path());
        }
    }
    for path in &stale {
        if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    let _ = fs::remove_dir(paths.fat.join("licenses"));
    if paths.app.is_dir() {
        fs::remove_dir_all(&paths.app)?;
    }
    if paths.script.is_file() {
        fs::remove_file(&paths.script)?;
    }
    if paths.script_constants.is_file() {
        fs::remove_file(&paths.script_constants)?;
    }
    let residue: Vec<_> = files
        .iter()
        .chain(stale.iter())
        .chain([&paths.app, &paths.script, &paths.script_constants])
        .filter(|path| path.exists())
        .collect();
    if residue.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "uninstall residue: {}",
            residue
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into())
    }
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

fn offer_reboot(paths: &Paths) -> Result<()> {
    println!("\nReboot now? Press A/Enter to reboot. Any other key exits without rebooting.");
    if read_event(paths)? != Some(InputEvent::Confirm) {
        println!("MiSTer MagiK: reboot skipped.");
        return Ok(());
    }
    reboot_now(paths)
}

fn reboot_now(paths: &Paths) -> Result<()> {
    if paths.test_mode() {
        println!("MiSTer MagiK: TEST: normal reboot requested.");
        return Ok(());
    }
    if !Command::new("sync").status()?.success() {
        return Err("sync failed".into());
    }
    let status = Command::new("reboot").status()?;
    if !status.success() {
        return Err("reboot command failed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    struct FailAt {
        step: WriteStep,
        suffix: &'static str,
    }

    impl WriteFaults for FailAt {
        fn check(&mut self, path: &Path, step: WriteStep) -> io::Result<()> {
            if step == self.step && path.ends_with(self.suffix) {
                Err(io::Error::other("injected write failure"))
            } else {
                Ok(())
            }
        }
    }

    fn fixture_paths(root: &Path) -> Paths {
        Paths {
            fat: root.to_path_buf(),
            inittab: root.join("inittab"),
            ini: root.join("MiSTer.ini"),
            backup: root.join("backup"),
            app: root.join("mister-magik"),
            manifest: root.join("manifest"),
            script: root.join("script"),
            script_constants: root.join("script.constants"),
            test_mode: true,
            test_keys: RefCell::default(),
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
        let app = &paths.app;
        let fpga = app.join("fpga");
        fs::create_dir_all(&fpga).unwrap();
        let files = [
            (paths.fat.join("MiSTer_MagiK"), b"main".as_slice()),
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
        fs::write(
            app.join("mister_magik_scanout_slots.metadata.txt"),
            format!(
                "module_sha256={module_sha}\nplatform_contract_sha256={contract}\nvermagic=5.15.1-MiSTer SMP\n"
            ),
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
        for (name, installed_path) in ManifestLayout::Public.paths().components() {
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
        fs::write(&paths.manifest, manifest).unwrap();
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
    fn stock_inittab_repair_is_idempotent() {
        let input = "x\n::sysinit:/media/fat/MiSTer_MagiK &\n::sysinit:/media/fat/MiSTer &\n::sysinit:/media/fat/MiSTer &\n";
        let mut output = Vec::new();
        let mut wrote = false;
        for line in input.lines() {
            if line.starts_with("::sysinit:/media/fat/MiSTer ") && line.ends_with('&') {
                if !wrote {
                    output.push("::sysinit:/media/fat/MiSTer &");
                    wrote = true;
                }
            } else if !line.starts_with("::sysinit:/media/fat/MiSTer_MagiK") {
                output.push(line);
            }
        }
        assert_eq!(output.join("\n"), "x\n::sysinit:/media/fat/MiSTer &");
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
    fn pending_file_collision_cannot_damage_the_original() {
        let root = env::temp_dir().join(format!("mister-manager-collision-{}", process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let target = root.join("MiSTer.ini");
        fs::write(&target, b"original\n").unwrap();
        let pending = root.join(format!(".MiSTer.ini.new.{}", process::id()));
        fs::write(&pending, b"hostile pending\n").unwrap();

        assert!(atomic_write(&target, b"replacement\n").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original\n");
        assert!(!pending.exists());
        fs::remove_dir_all(root).unwrap();
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
    fn every_write_boundary_rolls_back_all_replaced_files() {
        for (index, step) in [
            WriteStep::BeforeCreate,
            WriteStep::AfterWrite,
            WriteStep::AfterFlush,
            WriteStep::AfterPendingReadback,
            WriteStep::AfterRename,
            WriteStep::AfterFinalReadback,
            WriteStep::AfterDirectorySync,
        ]
        .into_iter()
        .enumerate()
        {
            let root =
                env::temp_dir().join(format!("mister-manager-rollback-{}-{index}", process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            let first = root.join("first");
            let second = root.join("second");
            fs::write(&first, b"first original").unwrap();
            fs::write(&second, b"second original").unwrap();
            let files = vec![
                PreparedFile::new(first.clone(), b"first replacement".to_vec()).unwrap(),
                PreparedFile::new(second.clone(), b"second replacement".to_vec()).unwrap(),
            ];
            let paths = fixture_paths(&root);
            let error = replace_transaction(
                &paths,
                &files,
                &mut FailAt {
                    step,
                    suffix: "second",
                },
                || Ok(()),
            )
            .unwrap_err();
            assert!(error.to_string().contains("rollback=complete"));
            assert_eq!(fs::read(&first).unwrap(), b"first original");
            assert_eq!(fs::read(&second).unwrap(), b"second original");
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn validation_failure_restores_files_and_removes_new_targets() {
        let root = env::temp_dir().join(format!("mister-manager-validation-{}", process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let existing = root.join("existing");
        let created = root.join("created");
        fs::write(&existing, b"original").unwrap();
        let files = vec![
            PreparedFile::new(existing.clone(), b"replacement".to_vec()).unwrap(),
            PreparedFile::new(created.clone(), b"new".to_vec()).unwrap(),
        ];
        let paths = fixture_paths(&root);
        assert!(
            replace_transaction(&paths, &files, &mut NoWriteFaults, || Err(
                "invalid result".into()
            ))
            .is_err()
        );
        assert_eq!(fs::read(existing).unwrap(), b"original");
        assert!(!created.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn confirmation_and_installed_action_routes_are_fail_closed() {
        let root = fixture_root("routing");
        let paths = fixture_paths(&root);

        queue(&paths, [InputEvent::Cancel]);
        let error = safety_confirmation(&paths, "warning", "installation").unwrap_err();
        assert_eq!(error.to_string(), "installation cancelled; no changes made");

        queue(&paths, [InputEvent::Down]);
        safety_confirmation(&paths, "warning", "installation").unwrap();

        queue(&paths, [InputEvent::Down, InputEvent::Confirm]);
        assert_eq!(
            choose_installed_action(&paths).unwrap(),
            Some(Action::Uninstall)
        );
        queue(&paths, [InputEvent::Up, InputEvent::Confirm]);
        assert_eq!(
            choose_installed_action(&paths).unwrap(),
            Some(Action::Uninstall)
        );
        queue(&paths, [InputEvent::Other]);
        assert_eq!(choose_installed_action(&paths).unwrap(), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_platform_preflight_preserves_boot_configuration() {
        let root = fixture_root("invalid-platform");
        let paths = fixture_paths(&root);
        fs::write(&paths.ini, b"[MiSTer]\nmain=MiSTer\n").unwrap();
        fs::write(&paths.inittab, b"::sysinit:/media/fat/MiSTer &\n").unwrap();
        fs::write(&paths.manifest, b"format=unsupported\n").unwrap();
        queue(&paths, [InputEvent::Down]);

        let error = install(&paths).unwrap_err();
        assert!(error.to_string().contains("platform verification failed"));
        assert_eq!(fs::read(&paths.ini).unwrap(), b"[MiSTer]\nmain=MiSTer\n");
        assert_eq!(
            fs::read(&paths.inittab).unwrap(),
            b"::sysinit:/media/fat/MiSTer &\n"
        );
        assert!(!paths.backup.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn test_mode_install_restore_and_uninstall_preserve_unowned_files() {
        let root = fixture_root("workflows");
        let paths = fixture_paths(&root);
        let original_ini = b"[MiSTer]\nmain=MiSTer\nvideo_mode=8\n";
        fs::write(&paths.ini, original_ini).unwrap();
        fs::write(
            &paths.inittab,
            b"::sysinit:/media/fat/MiSTer_MagiK &\n::sysinit:/media/fat/MiSTer &\n",
        )
        .unwrap();
        write_valid_platform(&paths);
        queue(&paths, [InputEvent::Down]);

        install(&paths).unwrap();
        assert!(selects_magik(&paths.ini).unwrap());
        assert_eq!(fs::read(&paths.backup).unwrap(), original_ini);
        validate_install(&paths).unwrap();

        restore(&paths).unwrap();
        assert_eq!(
            effective(&paths.ini, "MiSTer", "main").unwrap().as_deref(),
            Some("MiSTer")
        );
        validate_stock(&paths).unwrap();

        fs::write(root.join("unowned.txt"), b"keep").unwrap();
        fs::create_dir_all(root.join("Scripts")).unwrap();
        fs::write(&paths.script, b"owned").unwrap();
        fs::write(&paths.script_constants, b"owned").unwrap();
        queue(&paths, [InputEvent::Down]);
        uninstall(&paths).unwrap();
        assert!(!paths.app.exists());
        assert!(!paths.backup.exists());
        assert!(!paths.script.exists());
        assert!(!paths.script_constants.exists());
        assert_eq!(fs::read(root.join("unowned.txt")).unwrap(), b"keep");
        validate_stock(&paths).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_without_backup_removes_magik_selection_and_repairs_crlf_inittab() {
        let root = fixture_root("restore-no-backup");
        let paths = fixture_paths(&root);
        fs::write(&paths.ini, b"[MiSTer]\nmain=MiSTer_MagiK\n").unwrap();
        fs::write(
            &paths.inittab,
            b"::sysinit:/media/fat/mister-magik/boot.sh &\r\nother\r\n",
        )
        .unwrap();

        restore_stock(&paths).unwrap();
        assert!(!selects_magik(&paths.ini).unwrap());
        assert_eq!(
            fs::read(&paths.inittab).unwrap(),
            b"other\r\n::sysinit:/media/fat/MiSTer &\r\n"
        );
        validate_stock(&paths).unwrap();
        fs::remove_dir_all(root).unwrap();
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
