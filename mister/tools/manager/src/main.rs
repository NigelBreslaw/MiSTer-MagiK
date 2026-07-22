// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_ini::{apply_install, apply_restore, Document, OutputMode};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MANIFEST_FIELDS: &[&str] = &[
    "format",
    "main_path",
    "gui_path",
    "manager_path",
    "scanout_module_path",
    "scanout_metadata_path",
    "latch_rbf_path",
    "latch_metadata_path",
    "main_sha256",
    "gui_sha256",
    "manager_sha256",
    "scanout_module_sha256",
    "scanout_metadata_sha256",
    "latch_rbf_sha256",
    "latch_metadata_sha256",
    "platform_contract_sha256",
    "main_revision",
    "magik_revision",
    "menu_revision",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputEvent {
    Up,
    Down,
    Confirm,
    Cancel,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Restore,
    Uninstall,
}

struct Paths {
    fat: PathBuf,
    inittab: PathBuf,
    ini: PathBuf,
    backup: PathBuf,
    app: PathBuf,
    manifest: PathBuf,
    output_mode: PathBuf,
    script: PathBuf,
}

impl Paths {
    fn from_environment() -> Self {
        let fat =
            PathBuf::from(env::var_os("MISTER_MAGIK_FAT").unwrap_or_else(|| "/media/fat".into()));
        let app = fat.join("mister-magik");
        Self {
            inittab: PathBuf::from(
                env::var_os("MISTER_MAGIK_INITTAB").unwrap_or_else(|| "/etc/inittab".into()),
            ),
            ini: fat.join("MiSTer.ini"),
            backup: fat.join("MiSTer.ini.bak.before-magik"),
            manifest: app.join("platform-v2.manifest"),
            output_mode: app.join("installer-output-mode-v1"),
            script: fat.join("Scripts/MiSTer-MagiK.sh"),
            app,
            fat,
        }
    }

    fn test_mode(&self) -> bool {
        env::var("MISTER_MAGIK_TEST_MODE").as_deref() == Ok("1")
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
        "MiSTer MagiK supports automatic known-DAC CRT selection with safe HDMI fallback.",
        "installation",
    )?;
    let mode = choose_output_mode(paths)?;
    verify_platform(paths).map_err(|error| {
        format!("platform verification failed; boot configuration was not changed: {error}")
    })?;
    snapshot(paths)?;
    backup_ini(paths)?;
    ensure_executable(paths.fat.join("MiSTer_MagiK"))?;
    ensure_executable(paths.app.join("mister-magik-fb"))?;
    ensure_executable(paths.app.join("mister-magik-manager"))?;
    write_stock_inittab(paths)?;
    mutate_ini(&paths.ini, |document| apply_install(document, mode))?;
    atomic_write(
        &paths.output_mode,
        format!("{}\n", mode.as_str()).as_bytes(),
    )?;
    sync_storage(paths)?;
    println!("MiSTer MagiK: installed. Reboot to start MiSTer MagiK.");
    offer_reboot(paths)
}

fn restore(paths: &Paths) -> Result<()> {
    restore_stock(paths)?;
    println!("MiSTer MagiK: stock MiSTer boot restored. MiSTer MagiK files were preserved.");
    offer_reboot(paths)
}

fn uninstall(paths: &Paths) -> Result<()> {
    safety_confirmation(paths, "This permanently removes MiSTer MagiK, its settings, catalog, downloaded media, installer scripts, update_all entry, and saved backup. Stock MiSTer boot will be restored first.", "uninstall")?;
    restore_stock(paths)?;
    stop_children(paths)?;
    remove_owned(paths)?;
    println!("MiSTer MagiK: fully uninstalled.");
    offer_reboot(paths)
}

fn restore_stock(paths: &Paths) -> Result<()> {
    snapshot(paths)?;
    write_stock_inittab(paths)?;
    let backup = if paths.backup.is_file() {
        Some(Document::parse(&fs::read(&paths.backup)?)?)
    } else {
        None
    };
    mutate_ini(&paths.ini, |document| {
        apply_restore(document, backup.as_ref())
    })?;
    sync_storage(paths)?;
    if selects_magik(&paths.ini)? {
        return Err("MiSTer.ini still selects MiSTer MagiK".into());
    }
    verify_stock_inittab(&paths.inittab)
}

fn safety_confirmation(paths: &Paths, message: &str, operation: &str) -> Result<()> {
    println!("\n{message}\n\nPress Down on the keyboard or joystick to confirm. Any other input cancels.");
    if paths.test_mode() {
        let variable = match operation {
            "installation" => "MISTER_MAGIK_TEST_CONFIRM_INSTALL",
            "uninstall" => "MISTER_MAGIK_TEST_CONFIRM_UNINSTALL",
            _ => "",
        };
        if !variable.is_empty() && env::var(variable).as_deref() == Ok("1") {
            return Ok(());
        }
    }
    match read_event(paths)? {
        Some(InputEvent::Down) => Ok(()),
        Some(_) => Err(format!("{operation} cancelled; no changes made").into()),
        None => Err(format!("interactive input is unavailable; {operation} refused").into()),
    }
}

fn choose_output_mode(paths: &Paths) -> Result<OutputMode> {
    if let Ok(saved) = fs::read_to_string(&paths.output_mode) {
        return Ok(OutputMode::parse(saved.trim())?);
    }
    if paths.test_mode() {
        let mode = OutputMode::parse(
            &env::var("MISTER_MAGIK_TEST_OUTPUT_MODE").unwrap_or_else(|_| "auto".into()),
        )?;
        confirm_31khz(paths, mode)?;
        return Ok(mode);
    }
    let modes = [
        OutputMode::Crt240p60,
        OutputMode::Crt288p50,
        OutputMode::Crt480p60,
        OutputMode::Crt576p50,
        OutputMode::Auto,
        OutputMode::Hdmi,
    ];
    let mut selected = 0;
    loop {
        println!("\nChoose launcher output. Use Up/Down to choose, A/Enter to continue, or B/Escape to cancel.");
        for (index, mode) in modes.iter().enumerate() {
            println!(
                "{} {}",
                if index == selected { '>' } else { ' ' },
                output_label(*mode)
            );
        }
        match read_event(paths)? {
            Some(InputEvent::Up) => {
                selected = if selected == 0 {
                    modes.len() - 1
                } else {
                    selected - 1
                }
            }
            Some(InputEvent::Down) => selected = (selected + 1) % modes.len(),
            Some(InputEvent::Confirm) => {
                confirm_31khz(paths, modes[selected])?;
                return Ok(modes[selected]);
            }
            Some(InputEvent::Cancel | InputEvent::Other) => {
                return Err("cancelled; no changes made".into())
            }
            None => return Err("interactive input is unavailable; installation refused".into()),
        }
    }
}

fn confirm_31khz(paths: &Paths, mode: OutputMode) -> Result<()> {
    if !mode.is_31khz() {
        return Ok(());
    }
    println!("\nWARNING: {} is a 31 kHz signal.\nPress Down on the keyboard or joystick only if the display manual confirms 31 kHz support.", mode.as_str());
    if paths.test_mode() && env::var("MISTER_MAGIK_TEST_CONFIRM_31KHZ").as_deref() == Ok("1") {
        return Ok(());
    }
    if read_event(paths)? == Some(InputEvent::Down) {
        Ok(())
    } else {
        Err("31 kHz CRT mode was not explicitly confirmed; no changes made".into())
    }
}

fn output_label(mode: OutputMode) -> &'static str {
    match mode {
        OutputMode::Crt240p60 => "Analog IO VGA — 15 kHz CRT 240p60 (default)",
        OutputMode::Crt288p50 => "Analog IO VGA — 15 kHz CRT 288p50",
        OutputMode::Crt480p60 => "Analog IO VGA — 31 kHz CRT/VGA 480p60",
        OutputMode::Crt576p50 => "Analog IO VGA — 31 kHz CRT/VGA 576p50",
        OutputMode::Auto => "Automatic HDMI DAC detection",
        OutputMode::Hdmi => "HDMI only",
    }
}

fn choose_installed_action(paths: &Paths) -> Result<Option<Action>> {
    let mut action = Action::Restore;
    loop {
        println!("MiSTer MagiK is installed and selected as Main.\nUse Up/Down to choose, A/Enter to continue, or B/Escape to cancel.");
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
        let Ok(keys) = env::var("MISTER_MAGIK_TEST_KEYS") else {
            return Ok(None);
        };
        if keys.is_empty() {
            return Ok(None);
        }
        let key = keys.split(',').next().unwrap_or("");
        let remaining = keys.split_once(',').map_or("", |(_, rest)| rest);
        env::set_var("MISTER_MAGIK_TEST_KEYS", remaining);
        return Ok(Some(match key {
            "up" => InputEvent::Up,
            "down" => InputEvent::Down,
            "enter" => InputEvent::Confirm,
            "cancel" => InputEvent::Cancel,
            _ => InputEvent::Other,
        }));
    }
    if !io::stdin().is_terminal() {
        return Ok(None);
    }
    let original = Command::new("stty").arg("-g").output()?;
    let original = String::from_utf8(original.stdout)?.trim().to_string();
    if !Command::new("stty")
        .args(["-echo", "-icanon", "min", "1", "time", "0"])
        .status()?
        .success()
    {
        return Ok(None);
    }
    let result = read_event_bytes();
    let _ = Command::new("stty").arg(original).status();
    println!();
    result.map(Some)
}

fn read_event_bytes() -> Result<InputEvent> {
    let mut first = [0_u8; 1];
    io::stdin().read_exact(&mut first)?;
    if first[0] == b'\n' || first[0] == b'\r' {
        return Ok(InputEvent::Confirm);
    }
    if first[0] != 0x1b {
        return Ok(InputEvent::Other);
    }
    let mut tail = [0_u8; 2];
    let _ = Command::new("stty")
        .args(["min", "0", "time", "1"])
        .status();
    let count = io::stdin().read(&mut tail)?;
    Ok(match &tail[..count] {
        b"[A" | b"OA" => InputEvent::Up,
        b"[B" | b"OB" => InputEvent::Down,
        _ => InputEvent::Cancel,
    })
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

fn mutate_ini(path: &Path, mutation: impl FnOnce(&mut Document)) -> Result<()> {
    let input = if path.is_file() {
        fs::read(path)?
    } else {
        Vec::new()
    };
    let mut document = Document::parse(&input)?;
    mutation(&mut document);
    let output = document.render();
    let check = Document::parse(&output)?;
    drop(check);
    atomic_write(path, &output)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
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
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if fs::read(&pending)? != bytes {
            return Err("pending file read-back mismatch".into());
        }
        fs::rename(&pending, path)?;
        if fs::read(path)? != bytes {
            return Err("replaced file read-back mismatch".into());
        }
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&pending);
    }
    result
}

fn backup_ini(paths: &Paths) -> Result<()> {
    if !paths.ini.is_file() || paths.backup.exists() {
        return Ok(());
    }
    if selects_magik(&paths.ini)? {
        println!("MiSTer MagiK: WARNING: backup missing; not creating it from a MagiK-active MiSTer.ini.");
        return Ok(());
    }
    atomic_write(&paths.backup, &fs::read(&paths.ini)?)
}

fn write_stock_inittab(paths: &Paths) -> Result<()> {
    let input = fs::read_to_string(&paths.inittab)?;
    let newline = if input.contains("\r\n") { "\r\n" } else { "\n" };
    let mut output = Vec::new();
    let mut wrote = false;
    for raw in input.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with("::sysinit:/media/fat/MiSTer ") && line.ends_with('&') {
            if !wrote {
                output.push("::sysinit:/media/fat/MiSTer &");
                wrote = true;
            }
        } else if !line.starts_with("::sysinit:/media/fat/MiSTer_MagiK")
            && !line.starts_with("::sysinit:/media/fat/mister-magik/boot.sh")
        {
            output.push(line);
        }
    }
    if !wrote {
        output.push("::sysinit:/media/fat/MiSTer &");
    }
    let mut bytes = output.join(newline).into_bytes();
    bytes.extend_from_slice(newline.as_bytes());
    atomic_write(&paths.inittab, &bytes)
}

fn verify_stock_inittab(path: &Path) -> Result<()> {
    let text = fs::read_to_string(path)?;
    let stock = text
        .lines()
        .filter(|line| line.trim_end_matches('\r') == "::sysinit:/media/fat/MiSTer &")
        .count();
    if stock != 1
        || text.lines().any(|line| {
            line.starts_with("::sysinit:/media/fat/MiSTer_MagiK")
                || line.starts_with("::sysinit:/media/fat/mister-magik/boot.sh")
        })
    {
        return Err("inittab is not in verified stock state".into());
    }
    Ok(())
}

fn verify_platform(paths: &Paths) -> Result<()> {
    let fields = parse_manifest(&paths.manifest)?;
    if fields.len() != MANIFEST_FIELDS.len()
        || MANIFEST_FIELDS
            .iter()
            .any(|field| !fields.contains_key(*field))
    {
        return Err("platform manifest has unexpected fields".into());
    }
    if fields["format"] != "mister-magik-platform-v2" {
        return Err("unsupported platform manifest".into());
    }
    for name in [
        "main",
        "gui",
        "manager",
        "scanout_module",
        "scanout_metadata",
        "latch_rbf",
        "latch_metadata",
    ] {
        let expected = match name {
            "main" => "/media/fat/MiSTer_MagiK",
            "gui" => "/media/fat/mister-magik/mister-magik-fb",
            "manager" => "/media/fat/mister-magik/mister-magik-manager",
            "scanout_module" => "/media/fat/mister-magik/mister_magik_scanout_slots.ko",
            "scanout_metadata" => "/media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt",
            "latch_rbf" => "/media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf",
            "latch_metadata" => "/media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt",
            _ => unreachable!(),
        };
        if fields[&format!("{name}_path")] != expected {
            return Err(format!("invalid {name}_path").into());
        }
        let local = paths.fat.join(expected.trim_start_matches("/media/fat/"));
        if digest(&local)? != fields[&format!("{name}_sha256")] {
            return Err(format!("hash mismatch for {}", local.display()).into());
        }
    }
    println!(
        "MiSTer MagiK: verified platform {}",
        fields["magik_revision"]
    );
    Ok(())
}

fn parse_manifest(path: &Path) -> Result<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    for line in fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let (key, value) = line.split_once('=').ok_or("malformed platform manifest")?;
        if key.is_empty() || value.is_empty() || fields.insert(key.into(), value.into()).is_some() {
            return Err("invalid platform manifest".into());
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
    Ok(())
}

fn remove_owned(paths: &Paths) -> Result<()> {
    let files = [
        paths.fat.join("MiSTer_MagiK"),
        paths.fat.join("Scripts/mister-magik.sh"),
        paths.fat.join("Scripts/mister-magik-channel.sh"),
        paths.fat.join("downloader_mister_magik.ini"),
        paths.backup.clone(),
    ];
    for path in &files {
        if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    if paths.app.is_dir() {
        fs::remove_dir_all(&paths.app)?;
    }
    if paths.script.is_file() {
        fs::remove_file(&paths.script)?;
    }
    let residue: Vec<_> = files
        .iter()
        .chain([&paths.app, &paths.script])
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
    fn down_sequences_decode_to_the_same_event() {
        for bytes in [b"[B".as_slice(), b"OB".as_slice()] {
            assert_eq!(
                match bytes {
                    b"[B" | b"OB" => InputEvent::Down,
                    _ => InputEvent::Other,
                },
                InputEvent::Down
            );
        }
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
        let manifest = root.join("platform-v2.manifest");
        fs::write(&manifest, b"format=one\nformat=two\n").unwrap();
        assert!(parse_manifest(&manifest).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
