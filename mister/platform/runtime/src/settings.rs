// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Persistent MagiK settings stored on `/media/fat`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SETTINGS_VERSION: u32 = 1;

/// Physical orientation of the monitor used for the launcher.
///
/// Portrait variants describe how the monitor is mounted. The compositor
/// applies the inverse pixel rotation so launcher content remains upright.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenOrientation {
    #[default]
    Normal,
    MonitorClockwise,
    MonitorCounterclockwise,
}

impl ScreenOrientation {
    pub const ALL: [Self; 3] = [
        Self::Normal,
        Self::MonitorClockwise,
        Self::MonitorCounterclockwise,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::MonitorClockwise => "Monitor right (clockwise)",
            Self::MonitorCounterclockwise => "Monitor left (counterclockwise)",
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::MonitorClockwise => "monitor-clockwise",
            Self::MonitorCounterclockwise => "monitor-counterclockwise",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "normal" => Some(Self::Normal),
            "monitor-clockwise" | "clockwise" | "right" => Some(Self::MonitorClockwise),
            "monitor-counterclockwise" | "counterclockwise" | "left" => {
                Some(Self::MonitorCounterclockwise)
            }
            _ => None,
        }
    }

    pub const fn is_portrait(self) -> bool {
        !matches!(self, Self::Normal)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagikSettings {
    pub version: u32,
    #[serde(default)]
    pub screen_orientation: ScreenOrientation,
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
            screen_orientation: ScreenOrientation::Normal,
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
                Err(e) => match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(mut value) if value.get("version").and_then(|v| v.as_u64()) == Some(1) => {
                        if let Some(object) = value.as_object_mut() {
                            object.remove("screen_orientation");
                        }
                        match serde_json::from_value::<Self>(value) {
                            Ok(mut settings) => {
                                crate::ui_errln!(
                                    "settings: invalid screen orientation in {}, using normal",
                                    path.display()
                                );
                                settings.screen_orientation = ScreenOrientation::Normal;
                                settings.screensaver_delay_minutes =
                                    settings.screensaver_delay_minutes.clamp(1, 10);
                                settings
                            }
                            Err(_) => {
                                crate::ui_errln!(
                                    "settings: parse error in {}: {e}, using defaults",
                                    path.display()
                                );
                                Self::default()
                            }
                        }
                    }
                    _ => {
                        crate::ui_errln!(
                            "settings: parse error in {}: {e}, using defaults",
                            path.display()
                        );
                        Self::default()
                    }
                },
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                crate::ui_errln!("settings: read {}: {e}, using defaults", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let mut fault_control = crate::direct_reset_fault::process_fault_control();
        self.save_with_fault_control(&mut fault_control)
    }

    pub fn save_with_fault_control(
        &self,
        fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
    ) -> io::Result<()> {
        self.save_to_with_fault_control(
            mister_magik_catalog::device_layout::current_app_path("settings.json"),
            fault_control,
        )
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let mut fault_control = crate::direct_reset_fault::process_fault_control();
        self.save_to_with_fault_control(path, &mut fault_control)
    }

    pub fn save_to_with_fault_control(
        &self,
        path: impl AsRef<Path>,
        fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
    ) -> io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = temp_path(path);
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&tmp, text)?;
        mister_magik_catalog::fs_fault::maybe_fault_with_control(
            "settings.after_temp_write",
            path,
            fault_control,
        );
        fs::rename(&tmp, path)?;
        mister_magik_catalog::fs_fault::maybe_fault_with_control(
            "settings.after_rename",
            path,
            fault_control,
        );
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

    #[derive(Default)]
    struct RecordingFaultControl {
        points: Vec<String>,
    }

    impl mister_magik_catalog::fs_fault::DirectResetFaultControl for RecordingFaultControl {
        fn request_direct_reset(
            &mut self,
            request: &mister_magik_catalog::fs_fault::DirectResetFaultRequest,
        ) -> mister_magik_catalog::fs_fault::DirectResetFaultOutcome {
            self.points.push(request.point().to_string());
            mister_magik_catalog::fs_fault::DirectResetFaultOutcome::Noop
        }
    }

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
        assert_eq!(settings.screen_orientation, ScreenOrientation::Normal);
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
    fn settings_fault_hook_preserves_publication_order() {
        let path = temp_settings_path("fault-hook");
        let settings = MagikSettings::default();
        let mut control = RecordingFaultControl::default();

        settings
            .save_to_with_fault_control(&path, &mut control)
            .expect("save settings");

        assert_eq!(
            control.points,
            vec!["settings.after_temp_write", "settings.after_rename"]
        );
        let _ = fs::remove_dir_all(path.parent().expect("settings parent"));
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

    #[test]
    fn orientation_round_trips_with_stable_names() {
        for orientation in ScreenOrientation::ALL {
            let settings = MagikSettings {
                screen_orientation: orientation,
                ..MagikSettings::default()
            };
            let json = serde_json::to_string(&settings).unwrap();
            let decoded: MagikSettings = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded.screen_orientation, orientation);
        }
        assert_eq!(
            serde_json::to_string(&ScreenOrientation::MonitorClockwise).unwrap(),
            r#""monitor-clockwise""#
        );
    }

    #[test]
    fn legacy_version_one_settings_preserve_other_values() {
        let settings: MagikSettings = serde_json::from_str(
            r#"{"version":1,"simple_joystick_handling":true,"reduce_motion":true,"screensaver_enabled":false,"screensaver_delay_minutes":7}"#,
        )
        .unwrap();

        assert_eq!(settings.screen_orientation, ScreenOrientation::Normal);
        assert!(settings.simple_joystick_handling);
        assert!(settings.reduce_motion);
        assert!(!settings.screensaver_enabled);
        assert_eq!(settings.screensaver_delay_minutes, 7);
    }

    #[test]
    fn invalid_orientation_falls_back_without_discarding_settings() {
        let path = temp_settings_path("invalid-orientation");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"version":1,"screen_orientation":"sideways","reduce_motion":true}"#,
        )
        .unwrap();

        let settings = MagikSettings::load_from(path);

        assert_eq!(settings.screen_orientation, ScreenOrientation::Normal);
        assert!(settings.reduce_motion);
    }
}
