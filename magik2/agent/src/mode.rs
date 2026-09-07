//! Explicit boot selection. Changing the next boot never initiates a reboot.
use mister_magik_ini::Document;
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

const INI: &str = "/media/fat/MiSTer.ini";

fn load(path: &Path) -> Result<Document, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take((mister_magik_ini::MAX_INI_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Document::parse(&bytes).map_err(|e| e.to_string())
}

pub fn status() -> Result<Value, String> {
    Ok(
        json!({"configured_main":load(Path::new(INI))?.effective_value("MiSTer","main"),"running":crate::device::status().ok(),"activation":"next-explicit-reboot"}),
    )
}

pub fn set(fields: &serde_json::Map<String, Value>) -> Result<Value, String> {
    if fields.len() != 2 || fields.get("attended") != Some(&Value::Bool(true)) {
        return Err("mode-set requires mode and explicit attendance".into());
    }
    let main = match fields.get("mode").and_then(Value::as_str) {
        Some("dev") => "MiSTer_MagiKDev",
        Some("public") => "MiSTer_MagiK",
        Some("stock") => "MiSTer",
        _ => return Err("mode must be dev, public or stock".into()),
    };
    if main != "MiSTer" {
        let layout = if main == "MiSTer_MagiKDev" {
            "dev"
        } else {
            "public"
        };
        verify_platform(layout)?;
    } else if !Path::new("/media/fat/MiSTer").is_file() {
        return Err("stock Main is absent".into());
    }
    disarm()?;
    write_mode(Path::new(INI), main)?;
    status()
}

fn write_mode(path: &Path, main: &str) -> Result<(), String> {
    let mut document = load(path)?;
    document.set("MiSTer", "main", main);
    let temporary = path.with_extension("magik2-part");
    let mut file = File::create(&temporary).map_err(|e| e.to_string())?;
    file.write_all(&document.render())
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(&temporary, path).map_err(|e| e.to_string())?;
    File::open(path.parent().ok_or("INI parent absent")?)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    if load(path)?.effective_value("MiSTer", "main").as_deref() != Some(main) {
        return Err("boot mode verification failed".into());
    }
    Ok(())
}

pub fn verify_platform(layout: &str) -> Result<(), String> {
    use mister_magik_platform_manifest_contract::{Layout, ValidationProfile, parse};
    let layout = Layout::parse(layout).map_err(|e| e.to_string())?;
    let paths = layout.paths();
    let text = fs::read_to_string(paths.manifest).map_err(|e| e.to_string())?;
    let manifest =
        parse(&text, layout, ValidationProfile::AgentStrict).map_err(|e| e.to_string())?;
    for (name, path) in paths.components() {
        if crate::media::hash(Path::new(path))?
            != manifest
                .required(&format!("{name}_sha256"))
                .map_err(|e| e.to_string())?
        {
            return Err(format!("installed platform {name} hash mismatch"));
        }
    }
    Ok(())
}

pub fn disarm() -> Result<(), String> {
    for path in [
        "/media/fat/mister-magik/launcher.env",
        "/media/fat/mister-magik-dev/launcher.env",
        "/tmp/mister-magik/fs-fault-launcher.env",
        "/tmp/mister-magik/fs-fault-session",
        "/tmp/mister-magik/fs-fault.json",
        "/media/fat/mister-magik/rebuild-on-next-boot",
        "/media/fat/mister-magik-dev/rebuild-on-next-boot",
    ] {
        match fs::remove_file(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(format!("cannot disarm {path}: {e}")),
        }
        if Path::new(path).exists() {
            return Err(format!("arming remains at {path}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_selection_preserves_unrelated_ini_values() {
        let path = std::env::temp_dir().join(format!("magik2-mode-{}", std::process::id()));
        fs::write(
            &path,
            b"[MiSTer]\nmain=MiSTer\nuser=keep\n[Menu]\nvideo_mode=6\n",
        )
        .unwrap();
        write_mode(&path, "MiSTer_MagiKDev").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("user=keep"));
        assert!(text.contains("video_mode=6"));
        fs::remove_file(path).unwrap();
    }
}

pub fn reboot(fields: &serde_json::Map<String, Value>) -> Result<Value, String> {
    if fields.len() != 1 || fields.get("attended") != Some(&Value::Bool(true)) {
        return Err("reboot requires explicit attendance".into());
    }
    if Path::new("/tmp/mister-magik/reboot-unstable")
        .try_exists()
        .map_err(|e| e.to_string())?
    {
        return Err("device is reboot-unstable; use the documented SD-card recovery path".into());
    }
    disarm()?;
    let reply = crate::main_control::request("mister_magik_reboot\n")?;
    Ok(
        json!({"acknowledgement":reply,"reboot_requested":true,"arming":"clear","health":"not-yet-confirmed"}),
    )
}
