use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const DEFAULT_DROP_IN: &str = "/media/fat/downloader_mister_magik.ini";
const DEFAULT_RELEASE_MARKER: &str = "/media/fat/mister-magik/release-v1.txt";
const DB_SECTION: &str = "mister_magik";
const RELEASE_MARKER_KEY: &str = "mister-magik/release-v1.txt";
const BETA_DATABASE_URL: &str = "https://raw.githubusercontent.com/NigelBreslaw/MiSTer-MagiK/downloader/mister-magik-beta-db.json.zip";
const RELEASE_DATABASE_URL: &str = "https://raw.githubusercontent.com/NigelBreslaw/MiSTer-MagiK/downloader/mister-magik-release-db.json.zip";
const UPDATE_CHECK_RETRY_DELAYS: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_secs(10),
    Duration::from_secs(30),
];

pub(super) struct UpdateCheck {
    rx: Option<mpsc::Receiver<bool>>,
}

impl UpdateCheck {
    pub(super) fn start(enabled: bool) -> Self {
        if !enabled {
            return Self { rx: None };
        }
        let drop_in = std::env::var("MISTER_MAGIK_DOWNLOADER_INI")
            .unwrap_or_else(|_| DEFAULT_DROP_IN.to_string());
        let release_marker = std::env::var("MISTER_MAGIK_RELEASE_MARKER")
            .unwrap_or_else(|_| DEFAULT_RELEASE_MARKER.to_string());
        if !Path::new(&drop_in).is_file() || !Path::new(&release_marker).is_file() {
            return Self { rx: None };
        }

        let (tx, rx) = mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("magik-update-check".to_string())
            .spawn(move || {
                let mut last_error = None;
                for delay in UPDATE_CHECK_RETRY_DELAYS {
                    std::thread::sleep(delay);
                    match check_for_update(Path::new(&drop_in), Path::new(&release_marker)) {
                        Ok(available) => {
                            let _ = tx.send(available);
                            return;
                        }
                        Err(error) => last_error = Some(error),
                    }
                }
                if let Some(error) = last_error {
                    crate::ui_logln!("update check unavailable: {error}");
                }
                let _ = tx.send(false);
            });
        if let Err(error) = spawn {
            crate::ui_errln!("update check: failed to start worker: {error}");
            return Self { rx: None };
        }
        Self { rx: Some(rx) }
    }

    pub(super) fn try_recv(&mut self) -> Option<bool> {
        let result = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        Some(result)
    }
}

fn check_for_update(drop_in: &Path, release_marker: &Path) -> Result<bool, String> {
    let ini = fs::read_to_string(drop_in).map_err(|error| format!("read drop-in: {error}"))?;
    let configured_url = configured_db_url(&ini).ok_or_else(|| "missing db_url".to_string())?;
    let database_url =
        downloader_database_url(configured_url).ok_or_else(|| "unsupported db_url".to_string())?;
    let installed = fs::read_to_string(release_marker)
        .map_err(|error| format!("read release marker: {error}"))?;
    let installed_build = release_build_number(&installed)
        .ok_or_else(|| "invalid installed build number".to_string())?;
    let database = fetch_database(database_url)?;
    let remote_build = database_release_build_number(&database)
        .ok_or_else(|| "invalid database release build number".to_string())?;
    Ok(remote_build > installed_build)
}

fn configured_db_url(ini: &str) -> Option<&str> {
    let mut in_section = false;
    for raw_line in ini.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            in_section = section.trim() == DB_SECTION;
            continue;
        }
        if in_section {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key.trim() == "db_url" {
                let value = value.trim();
                return (!value.is_empty()).then_some(value);
            }
        }
    }
    None
}

fn downloader_database_url(url: &str) -> Option<&str> {
    match url {
        BETA_DATABASE_URL | RELEASE_DATABASE_URL => Some(url),
        _ => None,
    }
}

fn release_build_number(marker: &str) -> Option<u64> {
    marker
        .lines()
        .find_map(|line| line.strip_prefix("build_number="))?
        .parse()
        .ok()
}

fn database_release_build_number(database: &Value) -> Option<u64> {
    let url = database
        .get("files")?
        .get(RELEASE_MARKER_KEY)?
        .get("url")?
        .as_str()?;
    let (_, suffix) = url.split_once("/releases/download/")?;
    let (tag, _) = suffix.split_once('/')?;
    let version = tag.strip_prefix("v0.2.")?;
    version.parse().ok()
}

fn fetch_database(url: &str) -> Result<Value, String> {
    let archive = TempArchive::create()?;
    let mut command = Command::new("curl");
    command.args([
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--connect-timeout",
        "3",
        "--max-time",
        "8",
        "--max-filesize",
        "1048576",
    ]);
    if Path::new("/etc/ssl/certs/cacert.pem").is_file() {
        command.args(["--cacert", "/etc/ssl/certs/cacert.pem"]);
    }
    let status = command
        .arg(url)
        .stdout(Stdio::from(
            archive
                .file
                .try_clone()
                .map_err(|error| format!("clone temporary archive: {error}"))?,
        ))
        .status()
        .map_err(|error| format!("start curl: {error}"))?;
    if !status.success() {
        return Err(format!("curl exited with {status}"));
    }
    let mut unzip = Command::new("unzip")
        .args(["-p"])
        .arg(&archive.path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("start unzip: {error}"))?;
    let mut encoded = Vec::new();
    unzip
        .stdout
        .take()
        .ok_or_else(|| "missing unzip stdout".to_string())?
        .take(1_048_577)
        .read_to_end(&mut encoded)
        .map_err(|error| format!("read database: {error}"))?;
    if encoded.len() > 1_048_576 {
        let _ = unzip.kill();
        let _ = unzip.wait();
        return Err("database exceeds 1 MiB".to_string());
    }
    let status = unzip
        .wait()
        .map_err(|error| format!("wait for unzip: {error}"))?;
    if !status.success() {
        return Err(format!("unzip exited with {status}"));
    }
    serde_json::from_slice(&encoded).map_err(|error| format!("parse database: {error}"))
}

struct TempArchive {
    path: PathBuf,
    file: File,
}

impl TempArchive {
    fn create() -> Result<Self, String> {
        for sequence in 0..100u8 {
            let path = std::env::temp_dir().join(format!(
                "mister-magik-update-db-{}-{sequence}.zip",
                std::process::id()
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => return Ok(Self { path, file }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("create temporary archive: {error}")),
            }
        }
        Err("could not allocate temporary archive".to_string())
    }
}

impl Drop for TempArchive {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_the_mister_magik_database_url() {
        let ini = "[other]\ndb_url = https://example.test/nope\n\n[mister_magik]\ndb_url = https://raw.githubusercontent.com/Owner/Repo/downloader/mister-magik-beta-db.json.zip\n";
        assert_eq!(
            configured_db_url(ini),
            Some("https://raw.githubusercontent.com/Owner/Repo/downloader/mister-magik-beta-db.json.zip")
        );
    }

    #[test]
    fn accepts_only_canonical_channel_urls() {
        assert_eq!(
            downloader_database_url(BETA_DATABASE_URL),
            Some(BETA_DATABASE_URL)
        );
        assert_eq!(
            downloader_database_url(RELEASE_DATABASE_URL),
            Some(RELEASE_DATABASE_URL)
        );
        assert_eq!(
            downloader_database_url(
                "https://raw.githubusercontent.com/Owner/Repo/downloader/feed.json.zip"
            ),
            None
        );
    }

    #[test]
    fn extracts_release_build_number_from_database_asset_url() {
        let database = serde_json::json!({
            "files": {
                RELEASE_MARKER_KEY: {
                    "url": "https://github.com/Owner/Repo/releases/download/v0.2.19/mister-magik-release-v1.txt"
                }
            }
        });
        assert_eq!(database_release_build_number(&database), Some(19));
    }

    #[test]
    fn reads_installed_build_number() {
        assert_eq!(
            release_build_number("version=0.2.19\nbuild_number=19\n"),
            Some(19)
        );
    }
}
