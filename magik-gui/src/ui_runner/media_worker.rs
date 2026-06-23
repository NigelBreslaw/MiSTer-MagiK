use mister_magik_fb::media_update::{
    pack_status_from_state, parse_manifest_json, size_qualified_pack_path, state_path,
    valid_image_size, MediaUpdatePolicy, DEFAULT_ASSET_DIR, DEFAULT_IMAGE_SIZE,
    DEFAULT_MANIFEST_URL,
};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;

pub(super) fn start_screenshot_media_worker() -> Option<mpsc::Receiver<MediaWorkerMessage>> {
    let config = match MediaWorkerConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("screenshot media worker disabled: {error}");
            return None;
        }
    };
    if config.policy == MediaUpdatePolicy::Off {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("screenshot-media".to_string())
        .spawn(move || run_screenshot_media_worker(config, tx))
        .ok()?;
    Some(rx)
}

fn run_screenshot_media_worker(config: MediaWorkerConfig, tx: mpsc::Sender<MediaWorkerMessage>) {
    let _ = tx.send(MediaWorkerMessage::Timing {
        name: "screenshot_media_update_start".to_string(),
        detail: format!(
            "policy={} manifest_url={} image_size={} asset_dir={}",
            config.policy.label(),
            config.manifest_url,
            config.image_size,
            config.asset_dir.display()
        ),
    });
    let manifest_text = match fetch_manifest_text(&config.manifest_url) {
        Ok(text) => text,
        Err(error) => {
            let _ = tx.send(MediaWorkerMessage::Failed {
                detail: format!("manifest fetch failed: {error}"),
            });
            return;
        }
    };
    let manifest = match parse_manifest_json(&config.manifest_url, &manifest_text) {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = tx.send(MediaWorkerMessage::Failed {
                detail: format!("manifest parse failed: {error}"),
            });
            return;
        }
    };
    let state = read_media_state(&config.asset_dir);
    let mut counts = MediaCheckCounts::default();
    for pack in manifest
        .packs
        .iter()
        .filter(|pack| pack.image_size == config.image_size)
    {
        let local_path = match size_qualified_pack_path(
            &config.asset_dir.display().to_string(),
            &pack.id,
            &pack.image_size,
        ) {
            Ok(path) => PathBuf::from(path),
            Err(error) => {
                counts.failed += 1;
                let _ = tx.send(MediaWorkerMessage::PackStatus {
                    system: pack.id.clone(),
                    image_size: pack.image_size.clone(),
                    status: "failed".to_string(),
                    detail: error,
                });
                continue;
            }
        };
        let status = pack_status_from_state(pack, &local_path, state.as_ref());
        match status.label() {
            "current" => counts.current += 1,
            "missing" => counts.missing += 1,
            "stale" => counts.stale += 1,
            _ => counts.failed += 1,
        }
        let detail = match &status {
            mister_magik_fb::media_update::LocalPackStatus::Stale { reason } => {
                format!("local_path={} reason={reason}", local_path.display())
            }
            _ => format!("local_path={}", local_path.display()),
        };
        let _ = tx.send(MediaWorkerMessage::PackStatus {
            system: pack.id.clone(),
            image_size: pack.image_size.clone(),
            status: status.label().to_string(),
            detail,
        });
    }
    let _ = tx.send(MediaWorkerMessage::Done {
        detail: format!(
            "packs={} current={} missing={} stale={} failed={}",
            counts.total(),
            counts.current,
            counts.missing,
            counts.stale,
            counts.failed
        ),
    });
}

fn fetch_manifest_text(manifest_url: &str) -> Result<String, String> {
    let output = Command::new("wget")
        .arg("-q")
        .arg("--header")
        .arg("Accept-Encoding: identity")
        .arg("-O")
        .arg("-")
        .arg(manifest_url)
        .output()
        .map_err(|e| format!("spawn wget: {e}"))?;
    if !output.status.success() {
        return Err(format!("wget exited with {}", output.status));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("manifest utf8: {e}"))
}

fn read_media_state(asset_dir: &PathBuf) -> Option<Value> {
    let text = fs::read_to_string(state_path(&asset_dir.display().to_string())).ok()?;
    serde_json::from_str(&text).ok()
}

#[derive(Clone, Debug)]
struct MediaWorkerConfig {
    policy: MediaUpdatePolicy,
    manifest_url: String,
    image_size: String,
    asset_dir: PathBuf,
}

impl MediaWorkerConfig {
    fn from_env() -> Result<Self, String> {
        let policy =
            MediaUpdatePolicy::parse(std::env::var("MISTER_MEDIA_UPDATE").ok().as_deref())?;
        let image_size =
            std::env::var("MISTER_MEDIA_SIZE").unwrap_or_else(|_| DEFAULT_IMAGE_SIZE.to_string());
        if !valid_image_size(&image_size) {
            return Err(format!("invalid MISTER_MEDIA_SIZE: {image_size}"));
        }
        Ok(Self {
            policy,
            manifest_url: std::env::var("MISTER_MEDIA_MANIFEST_URL")
                .unwrap_or_else(|_| DEFAULT_MANIFEST_URL.to_string()),
            image_size,
            asset_dir: PathBuf::from(
                std::env::var("MISTER_MEDIA_ASSET_DIR")
                    .unwrap_or_else(|_| DEFAULT_ASSET_DIR.to_string()),
            ),
        })
    }
}

#[derive(Default)]
struct MediaCheckCounts {
    current: usize,
    missing: usize,
    stale: usize,
    failed: usize,
}

impl MediaCheckCounts {
    fn total(&self) -> usize {
        self.current + self.missing + self.stale + self.failed
    }
}

#[derive(Clone, Debug)]
pub(super) enum MediaWorkerMessage {
    Timing {
        name: String,
        detail: String,
    },
    PackStatus {
        system: String,
        image_size: String,
        status: String,
        detail: String,
    },
    Failed {
        detail: String,
    },
    Done {
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_worker_policy_defaults_to_download() {
        assert_eq!(
            MediaUpdatePolicy::parse(None).unwrap(),
            MediaUpdatePolicy::Download
        );
        assert_eq!(
            MediaUpdatePolicy::parse(Some("check-only")).unwrap(),
            MediaUpdatePolicy::Check
        );
        assert_eq!(
            MediaUpdatePolicy::parse(Some("off")).unwrap(),
            MediaUpdatePolicy::Off
        );
        assert!(MediaUpdatePolicy::parse(Some("maybe")).is_err());
    }
}
