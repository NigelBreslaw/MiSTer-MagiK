//! Test-only filesystem fault injection for destructive device experiments.

use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const POINT_ENV: &str = "MISTER_FS_FAULT_POINT";
const ACTION_ENV: &str = "MISTER_FS_FAULT_ACTION";
const DELAY_ENV: &str = "MISTER_FS_FAULT_DELAY_MS";
const DEFAULT_DELAY_MS: u64 = 2_000;
const MARKER_PATH: &str = "/tmp/mister-magik/fs-fault.json";
const MISTER_CMD: &str = "/dev/MiSTer_cmd";
const DIRECT_RESET_NO_SYNC: &str = "direct-reset-no-sync";
const DIRECT_RESET_NO_SYNC_CMD: &str = "mister_magik_direct_reset_no_sync\n";

#[derive(Clone, Debug, PartialEq, Eq)]
struct FaultConfig {
    point: String,
    action: String,
    delay_ms: u64,
}

impl FaultConfig {
    fn from_env() -> Option<Self> {
        let point = std::env::var(POINT_ENV).ok()?;
        if point.trim().is_empty() {
            return None;
        }
        let action = std::env::var(ACTION_ENV).unwrap_or_else(|_| DIRECT_RESET_NO_SYNC.to_string());
        let delay_ms = std::env::var(DELAY_ENV)
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_DELAY_MS);
        Some(Self {
            point,
            action,
            delay_ms,
        })
    }
}

pub fn maybe_fault(point: &str, target: impl AsRef<Path>) {
    let Some(config) = FaultConfig::from_env() else {
        return;
    };
    let target = target.as_ref();
    let mut runtime = SystemFaultRuntime;
    maybe_fault_with(point, target, &config, &mut runtime);
}

fn maybe_fault_with(
    point: &str,
    target: &Path,
    config: &FaultConfig,
    runtime: &mut impl FaultRuntime,
) {
    if config.point != point {
        return;
    }
    let _ = runtime.write_marker(MARKER_PATH, &marker_json(point, target, config));
    if config.action == DIRECT_RESET_NO_SYNC {
        let _ = runtime.send_mister_command(MISTER_CMD, DIRECT_RESET_NO_SYNC_CMD);
    } else {
        eprintln!(
            "fs_fault: unsupported action {} at point {}",
            config.action, point
        );
    }
    runtime.sleep_ms(config.delay_ms);
}

fn marker_json(point: &str, target: &Path, config: &FaultConfig) -> String {
    let value = json!({
        "schema": "mister-magik-fs-fault-v1",
        "point": point,
        "target": target.display().to_string(),
        "action": config.action,
        "delay_ms": config.delay_ms,
        "pid": std::process::id(),
        "ts_unix_ms": unix_ms_now(),
    });
    format!("{value}\n")
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

trait FaultRuntime {
    fn write_marker(&mut self, path: &str, json: &str) -> std::io::Result<()>;
    fn send_mister_command(&mut self, path: &str, command: &str) -> std::io::Result<()>;
    fn sleep_ms(&mut self, delay_ms: u64);
}

struct SystemFaultRuntime;

impl FaultRuntime for SystemFaultRuntime {
    fn write_marker(&mut self, path: &str, json: &str) -> std::io::Result<()> {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, json)
    }

    fn send_mister_command(&mut self, path: &str, command: &str) -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new().write(true).open(path)?;
        file.write_all(command.as_bytes())?;
        file.flush()
    }

    fn sleep_ms(&mut self, delay_ms: u64) {
        std::thread::sleep(Duration::from_millis(delay_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    #[derive(Default)]
    struct FakeRuntime {
        marker: Option<(String, String)>,
        command: Option<(String, String)>,
        slept_ms: Vec<u64>,
    }

    impl FaultRuntime for FakeRuntime {
        fn write_marker(&mut self, path: &str, json: &str) -> std::io::Result<()> {
            self.marker = Some((path.to_string(), json.to_string()));
            Ok(())
        }

        fn send_mister_command(&mut self, path: &str, command: &str) -> std::io::Result<()> {
            self.command = Some((path.to_string(), command.to_string()));
            Ok(())
        }

        fn sleep_ms(&mut self, delay_ms: u64) {
            self.slept_ms.push(delay_ms);
        }
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct EnvRestore {
        key: &'static str,
        value: Option<String>,
    }

    impl EnvRestore {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self {
                key,
                value: previous,
            }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::remove_var(key);
            Self {
                key,
                value: previous,
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.value {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn config_is_absent_without_point_env() {
        let _guard = env_lock();
        let _point = EnvRestore::remove(POINT_ENV);
        assert!(FaultConfig::from_env().is_none());
    }

    #[test]
    fn non_matching_point_is_noop() {
        let config = FaultConfig {
            point: "wanted".into(),
            action: DIRECT_RESET_NO_SYNC.into(),
            delay_ms: 7,
        };
        let mut runtime = FakeRuntime::default();
        maybe_fault_with(
            "other",
            &PathBuf::from("/media/fat/mister-magik/library.sqlite3"),
            &config,
            &mut runtime,
        );
        assert!(runtime.marker.is_none());
        assert!(runtime.command.is_none());
        assert!(runtime.slept_ms.is_empty());
    }

    #[test]
    fn matching_point_writes_marker_sends_reset_and_sleeps() {
        let config = FaultConfig {
            point: "catalog.sqlite.after_final_temp_sync".into(),
            action: DIRECT_RESET_NO_SYNC.into(),
            delay_ms: 42,
        };
        let mut runtime = FakeRuntime::default();
        maybe_fault_with(
            "catalog.sqlite.after_final_temp_sync",
            &PathBuf::from("/media/fat/mister-magik/library.sqlite3"),
            &config,
            &mut runtime,
        );
        let marker = runtime.marker.expect("marker");
        assert_eq!(marker.0, MARKER_PATH);
        assert!(marker.1.contains("catalog.sqlite.after_final_temp_sync"));
        assert!(marker.1.contains("/media/fat/mister-magik/library.sqlite3"));
        assert_eq!(
            runtime.command,
            Some((MISTER_CMD.to_string(), DIRECT_RESET_NO_SYNC_CMD.to_string()))
        );
        assert_eq!(runtime.slept_ms, vec![42]);
    }

    #[test]
    fn env_config_defaults_to_direct_reset_no_sync() {
        let _guard = env_lock();
        let _point = EnvRestore::set(POINT_ENV, "settings.after_rename");
        let _action = EnvRestore::remove(ACTION_ENV);
        let _delay = EnvRestore::remove(DELAY_ENV);
        let config = FaultConfig::from_env().expect("config");
        assert_eq!(config.action, DIRECT_RESET_NO_SYNC);
        assert_eq!(config.delay_ms, DEFAULT_DELAY_MS);
    }
}
