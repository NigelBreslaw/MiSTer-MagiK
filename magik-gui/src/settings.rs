//! Persistent MagiK settings stored on `/media/fat`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const SETTINGS_PATH: &str = "/media/fat/mister-magik/settings.json";

const SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagikSettings {
    pub version: u32,
    #[serde(default)]
    pub simple_joystick_handling: bool,
}

impl Default for MagikSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            simple_joystick_handling: false,
        }
    }
}

impl MagikSettings {
    pub fn load() -> Self {
        Self::load_from(SETTINGS_PATH)
    }

    pub fn load_from(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Self>(&text) {
                Ok(settings) if settings.version == SETTINGS_VERSION => settings,
                Ok(settings) => {
                    eprintln!(
                        "settings: unsupported version {} in {}, using defaults",
                        settings.version,
                        path.display()
                    );
                    Self::default()
                }
                Err(e) => {
                    eprintln!(
                        "settings: parse error in {}: {e}, using defaults",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                eprintln!("settings: read {}: {e}, using defaults", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to(SETTINGS_PATH)
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = temp_path(path);
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&tmp, text)?;
        mister_magik_catalog::fs_fault::maybe_fault("settings.after_temp_write", path);
        fs::rename(&tmp, path)?;
        mister_magik_catalog::fs_fault::maybe_fault("settings.after_rename", path);
        Ok(())
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("settings.json");
    tmp.set_file_name(format!("{name}.tmp"));
    tmp
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_settings_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("mister-magik-settings-{label}-{nanos}"))
            .join("settings.json")
    }

    #[test]
    fn missing_settings_loads_defaults() {
        let path = temp_settings_path("missing");

        assert_eq!(MagikSettings::load_from(path), MagikSettings::default());
    }

    #[test]
    fn valid_settings_loads_simple_joystick_flag() {
        let path = temp_settings_path("valid");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"version":1,"simple_joystick_handling":true,"future":"ignored"}"#,
        )
        .unwrap();

        let settings = MagikSettings::load_from(path);

        assert!(settings.simple_joystick_handling);
    }

    #[test]
    fn malformed_settings_falls_back_to_defaults() {
        let path = temp_settings_path("malformed");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{").unwrap();

        assert_eq!(MagikSettings::load_from(path), MagikSettings::default());
    }

    #[test]
    fn unsupported_settings_version_falls_back_to_defaults() {
        let path = temp_settings_path("unsupported");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"version":999,"simple_joystick_handling":true}"#).unwrap();

        assert_eq!(MagikSettings::load_from(path), MagikSettings::default());
    }

    #[test]
    fn saves_atomically_shaped_json() {
        let path = temp_settings_path("save");
        let settings = MagikSettings {
            simple_joystick_handling: true,
            ..MagikSettings::default()
        };

        settings.save_to(&path).unwrap();
        let loaded = MagikSettings::load_from(&path);

        assert_eq!(loaded, settings);
        assert!(!path.with_file_name("settings.json.tmp").exists());
    }
}
