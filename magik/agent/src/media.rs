//! Explicit media maintenance using the same manifest, identity and paths as MagiK.
use mister_magik_media_contract::{self as contract, update};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn fetch(url: &str, limit: usize) -> Result<Vec<u8>, String> {
    contract::validate_https_manifest_url(url)?;
    let output = crate::benchmark::execute_bounded(
        Command::new("curl").args([
            "--fail",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            "60",
            "--max-filesize",
            &limit.to_string(),
            url,
        ]),
        Duration::from_secs(65),
        || false,
        limit,
    );
    if output.code != Some(0) || output.error.is_some() {
        return Err(format!(
            "media fetch failed: {:?}; {}",
            output.error,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(output.stdout)
}

pub(crate) fn hash(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn download(url: &str, bytes: u64, sha256: &str, destination: &Path) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("unsupported media URL".into());
    }
    let temporary = destination.with_extension("magik-part");
    let result = (|| {
        let output = crate::benchmark::execute_bounded(
            Command::new("curl")
                .args([
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--proto",
                    "=http,https",
                    "--connect-timeout",
                    "10",
                    "--max-time",
                    "1200",
                    "--max-filesize",
                    &bytes.to_string(),
                    "--output",
                ])
                .arg(&temporary)
                .arg(url),
            Duration::from_secs(1205),
            || false,
            8192,
        );
        if output.code != Some(0) || output.error.is_some() {
            return Err(format!(
                "media download failed: {:?}; {}",
                output.error,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        if fs::metadata(&temporary).map_err(|e| e.to_string())?.len() != bytes
            || hash(&temporary)? != sha256
        {
            return Err("media size or SHA-256 mismatch; published file preserved".into());
        }
        File::open(&temporary)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(&temporary, destination).map_err(|e| e.to_string())?;
        File::open(destination.parent().ok_or("media parent missing")?)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    })();
    let _ = fs::remove_file(temporary);
    result
}

pub fn run(fields: &serde_json::Map<String, Value>) -> Result<Value, String> {
    if fields.len() != 3 {
        return Err("media requires action, layout and system".into());
    }
    let assets = match fields.get("layout").and_then(Value::as_str) {
        Some("dev") => "/media/fat/mister-magik-dev/assets",
        Some("public") => "/media/fat/mister-magik/assets",
        _ => return Err("invalid media layout".into()),
    };
    let action = fields
        .get("action")
        .and_then(Value::as_str)
        .ok_or("media action required")?;
    if !matches!(action, "check" | "download" | "qualify") {
        return Err("invalid media action".into());
    }
    let system = fields
        .get("system")
        .and_then(Value::as_str)
        .ok_or("system required")?;
    if !update::is_supported_pack_id(system) {
        return Err("unsupported media system".into());
    }
    let url = contract::DEFAULT_MANIFEST_URL;
    let bytes = fetch(url, contract::MAX_MANIFEST_BYTES as usize)?;
    let manifest =
        update::parse_manifest_json(url, std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)?;
    let pack = manifest
        .packs
        .iter()
        .find(|pack| pack.id == system)
        .ok_or("system absent from manifest")?;
    let path = PathBuf::from(update::size_qualified_pack_path(
        assets,
        system,
        &pack.image_size,
    )?);
    let state_path = PathBuf::from(update::state_path(assets));
    let mut state: Value = match File::open(&state_path).and_then(|file| {
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    }) {
        Ok(bytes) if bytes.len() <= 1024 * 1024 => serde_json::from_slice(&bytes)
            .map_err(|e| format!("invalid existing media state: {e}"))?,
        Ok(_) => return Err("media state exceeds 1 MiB".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({"schema":1,"systems":{}}),
        Err(e) => return Err(e.to_string()),
    };
    if !state.is_object() || !state["systems"].is_object() {
        return Err("invalid media state structure".into());
    }
    if state["systems"].get(system).is_none() {
        state["systems"][system] = json!({"packs":{}});
    }
    if !state["systems"][system].is_object() {
        return Err("invalid media system state".into());
    }
    if state["systems"][system].get("packs").is_none() {
        state["systems"][system]["packs"] = json!({});
    }
    if !state["systems"][system]["packs"].is_object() {
        return Err("invalid media packs state".into());
    }
    let before = update::pack_status_from_state(pack, &path, Some(&state));
    if action == "qualify" {
        let index = pack.index.as_ref().ok_or("pack index missing")?;
        if before != update::LocalPackStatus::Current
            || index.codec != "mmlz4b-index-v2"
            || hash(&path)? != pack.raw.sha256
            || hash(&update::index_path_for_pack_path(&path))? != index.sha256
        {
            return Err("screenshot media integrity qualification failed".into());
        }
    }
    if action == "download" {
        fs::create_dir_all(assets).map_err(|e| e.to_string())?;
        if before.requires_pack_download() {
            download(&pack.raw.url, pack.raw.bytes, &pack.raw.sha256, &path)?;
        }
        if let Some(index) = &pack.index
            && before.requires_index_download()
        {
            download(
                &index.url,
                index.bytes,
                &index.sha256,
                &update::index_path_for_pack_path(&path),
            )?;
        }
        let mut entry = json!({"version":pack.version,"image_size":pack.image_size,"sha256":pack.raw.sha256,"bytes":pack.raw.bytes,"local_path":path});
        if let Some(index) = &pack.index {
            entry["index"] = json!({"codec":index.codec,"object":index.object,"bytes":index.bytes,"sha256":index.sha256,"archive_bytes":index.archive_bytes,"archive_sha256":index.archive_sha256});
        }
        state["systems"][system]["packs"][&pack.image_size] = entry;
        state["systems"][system]["preferred_size"] = json!(pack.image_size);
        let temporary = state_path.with_extension("magik-part");
        let mut file = File::create(&temporary).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(&state).map_err(|e| e.to_string())?)
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(&temporary, &state_path).map_err(|e| e.to_string())?;
        File::open(assets)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(
        json!({"system":system,"layout":fields["layout"],"action":action,"manifest_authentication":"https","path":path,"before":format!("{before:?}"),"after":format!("{:?}",update::pack_status_from_state(pack,&path,Some(&state)))}),
    )
}
