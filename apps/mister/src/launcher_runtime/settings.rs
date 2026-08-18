// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Platform-selected persistence for shared launcher settings.

use crate::settings::{MagikSettings, ScreenOrientation};
use std::io;
use std::path::{Path, PathBuf};

pub trait SettingsStore {
    fn load(&self) -> MagikSettings;
    fn save(&self, settings: &MagikSettings) -> io::Result<()>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSettingsStore {
    path: PathBuf,
}

impl FileSettingsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SettingsStore for FileSettingsStore {
    fn load(&self) -> MagikSettings {
        MagikSettings::load_from(&self.path)
    }

    fn save(&self, settings: &MagikSettings) -> io::Result<()> {
        settings.save_to(&self.path)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmedOrientationStore {
    settings: FileSettingsStore,
    mister_ini_path: Option<PathBuf>,
}

impl ConfirmedOrientationStore {
    pub fn for_runtime(settings: FileSettingsStore) -> Self {
        #[cfg(all(target_os = "linux", target_arch = "arm"))]
        let mister_ini_path = Some(PathBuf::from(
            mister_magik_mister_runtime::display_resolution::DEVICE_INI_PATH,
        ));
        #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
        let mister_ini_path = None;
        Self {
            settings,
            mister_ini_path,
        }
    }

    #[cfg(test)]
    fn with_mister_ini_path(settings: FileSettingsStore, path: impl Into<PathBuf>) -> Self {
        Self {
            settings,
            mister_ini_path: Some(path.into()),
        }
    }

    pub fn reconcile_osd_rotation(&self, orientation: ScreenOrientation) -> io::Result<bool> {
        let Some(path) = self.mister_ini_path.as_deref() else {
            return Ok(false);
        };
        mister_magik_mister_runtime::display_resolution::persist_osd_rotation_to(
            path,
            osd_rotation_for_orientation(orientation),
        )
    }

    pub fn save_confirmed(
        &self,
        previous: &MagikSettings,
        confirmed: &MagikSettings,
    ) -> io::Result<()> {
        self.settings.save(confirmed)?;
        let Some(path) = self.mister_ini_path.as_deref() else {
            return Ok(());
        };
        if let Err(error) = mister_magik_mister_runtime::display_resolution::persist_osd_rotation_to(
            path,
            osd_rotation_for_orientation(confirmed.screen_orientation),
        ) {
            let settings_rollback = self.settings.save(previous);
            let ini_rollback =
                mister_magik_mister_runtime::display_resolution::persist_osd_rotation_to(
                    path,
                    osd_rotation_for_orientation(previous.screen_orientation),
                );
            return Err(io::Error::other(format!(
                "save MiSTer OSD rotation: {error}; settings rollback: {}; MiSTer.ini rollback: {}",
                rollback_status(settings_rollback),
                rollback_status(ini_rollback.map(|_| ()))
            )));
        }
        Ok(())
    }
}

pub const fn osd_rotation_for_orientation(orientation: ScreenOrientation) -> u8 {
    match orientation {
        ScreenOrientation::Normal => 0,
        ScreenOrientation::MonitorClockwise => 1,
        ScreenOrientation::MonitorCounterclockwise => 2,
    }
}

fn rollback_status(result: io::Result<()>) -> String {
    result.map_or_else(|error| error.to_string(), |()| "ok".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_store_round_trips_settings_at_injected_path() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let store = FileSettingsStore::new(
            std::env::temp_dir()
                .join(format!("mister-magik-settings-store-{stamp}"))
                .join("settings.json"),
        );
        let expected = MagikSettings {
            screensaver_enabled: false,
            screensaver_delay_minutes: 7,
            ..MagikSettings::default()
        };

        store.save(&expected).expect("save settings");

        assert_eq!(store.load(), expected);
    }

    #[test]
    fn confirmed_orientation_updates_settings_and_main_osd_rotation() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("mister-magik-orientation-store-{stamp}"));
        let settings_path = root.join("settings.json");
        let ini_path = root.join("MiSTer.ini");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&ini_path, "[MiSTer]\nosd_rotate=0 ; keep\n").unwrap();
        let settings = FileSettingsStore::new(&settings_path);
        let store = ConfirmedOrientationStore::with_mister_ini_path(settings.clone(), &ini_path);
        let previous = MagikSettings::default();
        settings.save(&previous).unwrap();
        let mut confirmed = previous.clone();
        confirmed.screen_orientation = ScreenOrientation::MonitorCounterclockwise;

        store.save_confirmed(&previous, &confirmed).unwrap();

        assert_eq!(settings.load(), confirmed);
        assert!(
            std::fs::read_to_string(&ini_path)
                .unwrap()
                .contains("osd_rotate=2 ; keep")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn orientation_mapping_matches_main_osd_rotation_direction() {
        assert_eq!(osd_rotation_for_orientation(ScreenOrientation::Normal), 0);
        assert_eq!(
            osd_rotation_for_orientation(ScreenOrientation::MonitorClockwise),
            1
        );
        assert_eq!(
            osd_rotation_for_orientation(ScreenOrientation::MonitorCounterclockwise),
            2
        );
    }

    #[test]
    fn failed_osd_write_restores_the_previous_launcher_setting() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("mister-magik-orientation-rollback-{stamp}"));
        let settings_path = root.join("settings.json");
        std::fs::create_dir_all(&root).unwrap();
        let settings = FileSettingsStore::new(&settings_path);
        let store = ConfirmedOrientationStore::with_mister_ini_path(
            settings.clone(),
            root.join("missing-MiSTer.ini"),
        );
        let previous = MagikSettings::default();
        settings.save(&previous).unwrap();
        let mut confirmed = previous.clone();
        confirmed.screen_orientation = ScreenOrientation::MonitorClockwise;

        assert!(store.save_confirmed(&previous, &confirmed).is_err());
        assert_eq!(settings.load(), previous);
        let _ = std::fs::remove_dir_all(root);
    }
}
