// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Persistent MagiK settings stored on `/media/fat`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagikSettings {
    pub version: u32,
    #[serde(default)]
    pub simple_joystick_handling: bool,
    #[serde(default)]
    pub reduce_motion: bool,
    #[serde(default = "default_screensaver_enabled")]
    pub screensaver_enabled: bool,
    #[serde(default = "default_screensaver_delay_minutes")]
    pub screensaver_delay_minutes: u8,
}

const fn default_screensaver_enabled() -> bool {
    true
}

const fn default_screensaver_delay_minutes() -> u8 {
    5
}

impl Default for MagikSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            simple_joystick_handling: false,
            reduce_motion: false,
            screensaver_enabled: default_screensaver_enabled(),
            screensaver_delay_minutes: default_screensaver_delay_minutes(),
        }
    }
}

impl MagikSettings {
    pub fn load() -> Self {
        Self::load_from(mister_magik_catalog::device_layout::current_app_path(
            "settings.json",
        ))
    }

    pub fn load_from(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Self>(&text) {
                Ok(mut settings) if settings.version == SETTINGS_VERSION => {
                    settings.screensaver_delay_minutes =
                        settings.screensaver_delay_minutes.clamp(1, 10);
                    settings
                }
                Ok(settings) => {
                    crate::ui_errln!(
                        "settings: unsupported version {} in {}, using defaults",
                        settings.version,
                        path.display()
                    );
                    Self::default()
                }
                Err(e) => {
                    crate::ui_errln!(
                        "settings: parse error in {}: {e}, using defaults",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                crate::ui_errln!("settings: read {}: {e}, using defaults", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to(mister_magik_catalog::device_layout::current_app_path(
            "settings.json",
        ))
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
        assert!(!settings.reduce_motion);
        assert!(settings.screensaver_enabled);
        assert_eq!(settings.screensaver_delay_minutes, 5);
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
            reduce_motion: true,
            ..MagikSettings::default()
        };

        settings.save_to(&path).unwrap();
        let loaded = MagikSettings::load_from(&path);

        assert_eq!(loaded, settings);
        assert!(!path.with_file_name("settings.json.tmp").exists());
    }

    #[test]
    fn screensaver_delay_is_clamped_to_supported_range() {
        let path = temp_settings_path("delay-clamp");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"version":1,"screensaver_enabled":false,"screensaver_delay_minutes":99}"#,
        )
        .unwrap();

        let settings = MagikSettings::load_from(path);

        assert!(!settings.screensaver_enabled);
        assert_eq!(settings.screensaver_delay_minutes, 10);
    }
}
