//! Fixed native service boot registration. Never changes boot mode or reboots.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const HOOK: &str = "# MiSTer MagiK native service boot v1\n/bin/sh /media/fat/mister-magik2/start-service.sh || :\n# End MiSTer MagiK native service boot v1\n";
const SCRIPT: &str = "#!/bin/sh\nset -eu\nexport MISTER_MAGIK2_INSTALL_ROOT=/media/fat/mister-magik2\nexport MISTER_MAGIK2_STATE_ROOT=/tmp/mister-magik2\nMISTER_MAGIK2_TOKEN=$(cat /media/fat/mister-magik2/token)\nexport MISTER_MAGIK2_TOKEN\n[ -n \"$MISTER_MAGIK2_TOKEN\" ]\nmkdir -p /tmp/mister-magik2\nnohup /media/fat/mister-magik2/mister-magik-service </dev/null >>/tmp/mister-magik2/agent.log 2>&1 &\n";

fn regular(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            Err(io::Error::other("boot path is not a regular file"))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn atomic(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    regular(path)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let temporary = path.with_extension(format!("magik-next-{}-{nonce}", std::process::id()));
    // Unique siblings leave an interrupted write harmless; never follow links.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(data)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(path.parent().unwrap())?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn startup(original: &str) -> io::Result<String> {
    let (header, rest) = if original.starts_with("#!") {
        original
            .split_once('\n')
            .ok_or_else(|| io::Error::other("invalid startup shebang"))?
    } else {
        ("#!/bin/sh", original)
    };
    if rest.starts_with(HOOK) {
        return Ok(original.to_owned());
    }
    if original.contains("MiSTer MagiK native service boot")
        || original.contains("mister-magik2/start-service.sh")
    {
        return Err(io::Error::other(
            "unrecognized service boot hook; inspect startup file",
        ));
    }
    Ok(format!("{header}\n{HOOK}{rest}"))
}

pub fn ready(fat: &Path, init: &Path) -> bool {
    let startup_path = fat.join("linux/user-startup.sh");
    let script = fat.join("mister-magik2/start-service.sh");
    init.is_file()
        && fs::metadata(fat.join("mister-magik2/mister-magik-service"))
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        && fs::read_to_string(fat.join("mister-magik2/token")).is_ok_and(|t| !t.trim().is_empty())
        && regular(&startup_path).is_ok()
        && regular(&script).is_ok()
        && fs::read_to_string(&script).ok().as_deref() == Some(SCRIPT)
        && fs::read_to_string(&startup_path).ok().is_some_and(|text| {
            text.split_once('\n')
                .is_some_and(|(_, rest)| rest.starts_with(HOOK))
        })
        && fs::metadata(startup_path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

pub fn install(fat: &Path, init: &Path) -> io::Result<()> {
    if !init.is_file() {
        return Err(io::Error::other("MiSTer S99user boot hook is unavailable"));
    }
    let root = fat.join("mister-magik2");
    if !root.join("mister-magik-service").is_file() || fs::read(root.join("token"))?.is_empty() {
        return Err(io::Error::other(
            "native service binary or token unavailable",
        ));
    }
    let path = fat.join("linux/user-startup.sh");
    regular(&path)?;
    let original = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => "#!/bin/sh\n".to_owned(),
        Err(error) => return Err(error),
    };
    let updated = startup(&original)?;
    fs::create_dir_all(path.parent().unwrap())?;
    if updated != original {
        let backup = root.join("user-startup.before-magik");
        if !backup.exists() {
            atomic(&backup, original.as_bytes(), 0o600)?;
        }
    }
    if fs::read_to_string(root.join("start-service.sh"))
        .ok()
        .as_deref()
        != Some(SCRIPT)
    {
        atomic(&root.join("start-service.sh"), SCRIPT.as_bytes(), 0o700)?;
    }
    if !ready(fat, init) {
        let mode = fs::metadata(&path)
            .map(|m| m.permissions().mode())
            .unwrap_or(0o755)
            | 0o100;
        atomic(&path, updated.as_bytes(), mode)?;
    }
    if !ready(fat, init) {
        return Err(io::Error::other("service boot registration did not verify"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_preserves_user_commands_and_precedes_exit() {
        let original = "#!/bin/bash\necho user\nexit 0\n";
        let changed = startup(original).unwrap();
        assert!(changed.ends_with("echo user\nexit 0\n"));
        assert_eq!(startup(&changed).unwrap(), changed);
        assert!(startup("#!/bin/sh\n# MiSTer MagiK native service boot unknown\n").is_err());
    }

    #[test]
    fn installs_verifies_and_preserves_backup() {
        let directory =
            std::env::temp_dir().join(format!("magik-service-boot-test-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let fat = directory.as_path();
        fs::create_dir_all(fat.join("mister-magik2")).unwrap();
        fs::create_dir_all(fat.join("linux")).unwrap();
        fs::write(fat.join("mister-magik2/token"), "token").unwrap();
        fs::write(fat.join("mister-magik2/mister-magik-service"), "binary").unwrap();
        fs::set_permissions(
            fat.join("mister-magik2/mister-magik-service"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let init = fat.join("S99user");
        assert!(install(fat, &init).is_err());
        fs::write(&init, "init").unwrap();
        fs::write(fat.join("linux/user-startup.sh"), "#!/bin/bash\nexit 0\n").unwrap();
        install(fat, &init).unwrap();
        assert!(ready(fat, &init));
        install(fat, &init).unwrap();
        assert_eq!(
            fs::read_to_string(fat.join("mister-magik2/user-startup.before-magik")).unwrap(),
            "#!/bin/bash\nexit 0\n"
        );
        fs::write(fat.join("mister-magik2/start-service.sh"), "corrupt").unwrap();
        assert!(!ready(fat, &init));
        // An interrupted atomic write must not block a subsequent registration.
        fs::write(
            fat.join("linux/user-startup.magik-next-abandoned"),
            "partial",
        )
        .unwrap();
        install(fat, &init).unwrap();
        assert!(ready(fat, &init));
        assert!(
            std::process::Command::new("/bin/sh")
                .arg("-n")
                .arg(fat.join("mister-magik2/start-service.sh"))
                .status()
                .unwrap()
                .success()
        );
        // Never replace an operator's symlink or its target.
        fs::remove_file(fat.join("linux/user-startup.sh")).unwrap();
        fs::write(fat.join("operator-startup"), "preserve").unwrap();
        std::os::unix::fs::symlink(
            fat.join("operator-startup"),
            fat.join("linux/user-startup.sh"),
        )
        .unwrap();
        assert!(install(fat, &init).is_err());
        assert_eq!(
            fs::read_to_string(fat.join("operator-startup")).unwrap(),
            "preserve"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
